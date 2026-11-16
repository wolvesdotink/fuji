import {
  computed,
  onMounted,
  onScopeDispose,
  ref,
  watch,
  type CSSProperties,
  type Ref,
} from "vue";
import {
  anchorStart,
  clamp,
  clampStart,
  createFrameCommit,
  wheelZoomFactor,
} from "@/lib/zoomPan";
import { registerLoupe, unregisterLoupe, type LoupeHandle } from "@/lib/loupeBus";

/**
 * Loupe for the single-image viewers: fit ↔ 100% toggle, cursor-anchored
 * wheel/pinch zoom, and clamped drag-pan on the full-resolution `<img>`.
 *
 * Two kinds of state, with different lifetimes:
 *
 * - The **view zoom** (`zoom`, `centerX`, `centerY`) belongs to the viewer and
 *   is carried across navigation, Lightroom-style: `zoom` is device pixels
 *   per image pixel (1 = 100%) or null for fit, and the centre is the image
 *   point at the middle of the view as a fraction of the image. The CSS
 *   transform is derived from it plus the current photo's layout, so it
 *   survives resizes, display changes and neighbours of another size.
 *
 * - Everything tied to one photo lives in a `PhotoRecord`: which `<img>` it
 *   is, its measured layout, whether its full-resolution decode is done, and
 *   input received before it could be zoomed. The record is replaced
 *   wholesale when the photo changes, so nothing from one photo (a late load
 *   event, a queued wheel zoom, a finishing decode) can land on the next.
 *
 * At fit the `<img>` keeps its normal fitted layout and gets no inline
 * styles at all, so the fitted view renders exactly as before. Zoomed, it is
 * laid out once at its natural size and moved with `translate3d() scale()`
 * (see `zoomedStyle`), so panning and zooming never trigger layout.
 *
 * The 600 px thumbnail placeholder is never zoomed. Zoom input that arrives
 * before the full-res image has loaded is queued on the photo's record and
 * replayed in order once its size is known, through the same clamped path
 * as live input; meanwhile the indicator says the full-resolution image is
 * still loading.
 */

/** Highest zoom: one image pixel spread over 4×4 device pixels (400%). */
const MAX_ZOOM = 4;
/** Click / Z target: one image pixel per device pixel. */
const TOGGLE_ZOOM = 1;
/** Trackpad pinch reports small deltas; scale them up to feel direct. */
const PINCH_SENSITIVITY = 0.01;
/** Pointer travel (CSS px) after which a press counts as a drag, not a click. */
const DRAG_THRESHOLD = 3;
/** Zoom within this ratio of fit counts as fit. */
const FIT_EPSILON = 1e-3;
/** Most zoom steps queued for a photo that hasn't loaded yet. */
const MAX_QUEUED = 64;
/**
 * How long a photo must stay on screen before its full-resolution decode
 * starts (unless zoom is already engaged). Arrowing through a card doesn't
 * queue a 26 MP decode per photo.
 */
const DECODE_DWELL_MS = 300;

interface Layout {
  /** Viewport (stage) size in CSS px. */
  vw: number;
  vh: number;
  /** Stage origin in the viewport (CSS px), for device-pixel snapping. */
  ox: number;
  oy: number;
  /** Image size in image pixels, in display orientation. */
  natW: number;
  natH: number;
  /** CSS px per image px when fitted (≤ 1: images are never upscaled). */
  fit: number;
  dpr: number;
}

/**
 * Zoom input received before the photo could be zoomed. `toggle`: click / Z
 * at fit ("100% here"); `scale`: one wheel or pinch step. (fx, fy) is the
 * anchor as a fraction of the stage.
 */
type QueuedInput =
  | { kind: "toggle"; fx: number; fy: number }
  | { kind: "scale"; factor: number; fx: number; fy: number };

interface PhotoRecord {
  /** Photo identity (the viewer's image id). */
  id: string | undefined;
  /** The full-res `<img>` this record measured; null until it exists. */
  el: HTMLImageElement | null;
  layout: Layout | null;
  /** Full-resolution decode finished (or failed; then the browser decodes
   *  on demand). */
  decoded: boolean;
  decodeRequested: boolean;
  queued: QueuedInput[];
}

const newRecord = (id: string | undefined): PhotoRecord => ({
  id,
  el: null,
  layout: null,
  decoded: false,
  decodeRequested: false,
  queued: [],
});

// Full-resolution decodes run one at a time across the app. A queued job
// whose photo has been left by the time its turn comes is skipped, and a
// running one that is superseded has its result dropped by the caller.
let decodeChain: Promise<void> = Promise.resolve();

export interface LoupeOptions {
  /** The element zoom input is bound to (the viewer's image container). */
  stage: Ref<HTMLElement | null>;
  /** Identity of the photo on screen. */
  photoId: Ref<string | undefined>;
  /** The full-resolution `<img>`. */
  image: Ref<HTMLImageElement | null>;
  /** True once the current full-resolution image has loaded. */
  ready: Ref<boolean>;
  /** False for videos: the loupe resets and ignores all input. */
  enabled: Ref<boolean>;
}

interface GestureLikeEvent extends UIEvent {
  scale: number;
  clientX: number;
  clientY: number;
}

export function useLoupe({ stage, photoId, image, ready, enabled }: LoupeOptions) {
  // Committed (reactive) view state — drives the transform and indicator.
  const zoom = ref<number | null>(null);
  const centerX = ref(0.5);
  const centerY = ref(0.5);
  const queuedKind = ref<"toggle" | "scale" | null>(null);
  const dragging = ref(false);
  /** Bumped whenever the current record's layout or decode state changes. */
  const recordVersion = ref(0);

  // Pending view state, written synchronously by input handlers and
  // committed once per frame (see createFrameCommit).
  let pZoom: number | null = null;
  let pCx = 0.5;
  let pCy = 0.5;

  let rec: PhotoRecord = newRecord(photoId.value);

  const kindOf = (r: PhotoRecord) =>
    r.queued.length === 0
      ? null
      : r.queued.some((q) => q.kind === "scale")
        ? "scale"
        : "toggle";

  const frame = createFrameCommit(() => {
    zoom.value = pZoom;
    centerX.value = pCx;
    centerY.value = pCy;
    queuedKind.value = kindOf(rec);
  });

  // --- geometry -----------------------------------------------------------

  const fitZoom = (l: Layout) => l.fit * l.dpr;
  const maxZoom = (l: Layout) => Math.max(MAX_ZOOM, fitZoom(l) * 2);
  // Images too small to reach 100% at fit (rare for camera files) still get
  // a useful toggle: twice their fitted size.
  const toggleZoom = (l: Layout) =>
    fitZoom(l) < TOGGLE_ZOOM ? TOGGLE_ZOOM : Math.min(maxZoom(l), fitZoom(l) * 2);

  /** Displayed span (start, length) per axis for a zoom + centre. */
  function spans(l: Layout, z: number, cx: number, cy: number) {
    const lenX = (l.natW * z) / l.dpr;
    const lenY = (l.natH * z) / l.dpr;
    return {
      lenX,
      lenY,
      sx: clampStart(l.vw / 2 - cx * lenX, lenX, l.vw),
      sy: clampStart(l.vh / 2 - cy * lenY, lenY, l.vh),
    };
  }

  /** Store a zoom + displayed span starts as pending state (clamped). */
  function setFromSpans(l: Layout, z: number, sx: number, sy: number) {
    const lenX = (l.natW * z) / l.dpr;
    const lenY = (l.natH * z) / l.dpr;
    pZoom = z;
    pCx = (l.vw / 2 - clampStart(sx, lenX, l.vw)) / lenX;
    pCy = (l.vh / 2 - clampStart(sy, lenY, l.vh)) / lenY;
  }

  function setFit() {
    pZoom = null;
    pCx = 0.5;
    pCy = 0.5;
    rec.queued = [];
  }

  /** Zoom to `target` (clamped), keeping the image point under (ax, ay). */
  function zoomAt(l: Layout, target: number, ax: number, ay: number) {
    const z = pZoom ?? fitZoom(l);
    const next = clamp(target, fitZoom(l), maxZoom(l));
    if (next <= fitZoom(l) * (1 + FIT_EPSILON)) {
      setFit();
      return;
    }
    const cur = spans(l, z, pCx, pCy);
    const ratio = next / z;
    setFromSpans(l, next, anchorStart(cur.sx, ax, ratio), anchorStart(cur.sy, ay, ratio));
  }

  /** Re-validate a kept zoom against the current photo and viewport. */
  function revalidate(l: Layout) {
    if (pZoom === null) return;
    const z = clamp(pZoom, fitZoom(l), maxZoom(l));
    if (z <= fitZoom(l) * (1 + FIT_EPSILON)) {
      setFit();
    } else {
      const s = spans(l, z, pCx, pCy);
      setFromSpans(l, z, s.sx, s.sy);
    }
  }

  /** Apply one zoom step now (`l` known) — the single path for live and
   *  replayed input, so both clamp per step identically. */
  function applyInput(l: Layout, q: QueuedInput) {
    const ax = q.fx * l.vw;
    const ay = q.fy * l.vh;
    if (q.kind === "toggle") {
      if (pZoom === null) zoomAt(l, toggleZoom(l), ax, ay);
      else setFit();
    } else {
      zoomAt(l, (pZoom ?? fitZoom(l)) * q.factor, ax, ay);
    }
  }

  /** Route an input to the live path or this photo's queue. */
  function input(q: QueuedInput) {
    const l = rec.layout;
    if (l) {
      applyInput(l, q);
    } else {
      rec.queued.push(q);
      // Bounded: fold the oldest two scale steps into one if it grows.
      if (rec.queued.length > MAX_QUEUED) {
        const i = rec.queued.findIndex((x) => x.kind === "scale");
        const j = i < 0 ? -1 : rec.queued.findIndex((x, k) => k > i && x.kind === "scale");
        if (i >= 0 && j >= 0) {
          const a = rec.queued[i] as Extract<QueuedInput, { kind: "scale" }>;
          const b = rec.queued[j] as Extract<QueuedInput, { kind: "scale" }>;
          rec.queued.splice(j, 1, { ...b, factor: a.factor * b.factor });
          rec.queued.splice(i, 1);
        } else {
          rec.queued.shift();
        }
      }
      // From fit, a queue that never gets above fit (clamping at fit, the
      // one bound known before load) can only replay to fit: drop it rather
      // than show a loading state for a zoom that won't happen. The upper
      // bound only ever lowers the result, so this is exact.
      if (pZoom === null && rec.queued.every((x) => x.kind === "scale")) {
        let level = 1;
        for (const x of rec.queued) {
          if (x.kind === "scale") level = Math.max(1, level * x.factor);
        }
        if (level <= 1 + FIT_EPSILON) rec.queued = [];
      }
    }
    // Zoom intent: don't make the user wait out the decode dwell.
    requestDecode(true);
  }

  function measure() {
    const img = rec.el;
    const el = stage.value;
    // The element itself is the source of truth: loaded, with a size.
    if (!img || !el || !ready.value || !img.complete || !img.naturalWidth || !img.naturalHeight) {
      rec.layout = null;
      recordVersion.value++;
      return;
    }
    const rect = el.getBoundingClientRect();
    let natW = img.naturalWidth;
    let natH = img.naturalHeight;
    // Defensive: if the laid-out box is oriented differently from the
    // natural size (EXIF rotation reported unrotated), follow the layout.
    const ow = img.offsetWidth;
    const oh = img.offsetHeight;
    if (ow > 0 && oh > 0 && natW !== natH && ow !== oh && natW > natH !== ow > oh) {
      [natW, natH] = [natH, natW];
    }
    const l: Layout = {
      vw: rect.width,
      vh: rect.height,
      ox: rect.left,
      oy: rect.top,
      natW,
      natH,
      fit: Math.min(1, rect.width / natW, rect.height / natH),
      dpr: window.devicePixelRatio || 1,
    };
    if (l.vw === 0 || l.vh === 0) return;
    rec.layout = l;

    revalidate(l);
    // Replay this photo's queued input, in order, through the live path.
    const queued = rec.queued;
    rec.queued = [];
    for (const q of queued) applyInput(l, q);

    frame.flush();
    recordVersion.value++;
  }

  // --- full-resolution decode --------------------------------------------
  //
  // WebKit decodes a large image only at the size it is displayed, so the
  // fitted view holds a subsampled bitmap, and laying the image out at
  // natural size for the first zoom would force a synchronous full decode
  // on the next paint. So the current photo is decoded at full size off the
  // main thread — after it has stayed on screen for DECODE_DWELL_MS, or at
  // once when zoom is engaged — one decode at a time app-wide. Zoom is shown
  // once its photo's decode is done; until then it reads as loading.

  let dwellTimer: number | null = null;

  function clearDwell() {
    if (dwellTimer !== null) {
      clearTimeout(dwellTimer);
      dwellTimer = null;
    }
  }

  /** `now`: start without waiting for the dwell (zoom is engaged). */
  function requestDecode(now: boolean) {
    const r = rec;
    const img = r.el;
    if (r.decodeRequested || !r.layout || !img) return;
    if (!now) {
      if (dwellTimer === null) {
        dwellTimer = window.setTimeout(() => {
          dwellTimer = null;
          requestDecode(true);
        }, DECODE_DWELL_MS);
      }
      return;
    }
    clearDwell();
    r.decodeRequested = true;
    decodeChain = decodeChain.then(async () => {
      // Left this photo while waiting for our turn: skip the work entirely.
      if (r !== rec || r.el !== img) return;
      await img.decode().catch(() => {});
      // Superseded while decoding: drop the result.
      if (r !== rec || r.el !== img) return;
      r.decoded = true;
      // Re-measure: layout may have changed while decoding, and this is the
      // moment a waiting zoom becomes visible.
      measure();
    });
  }

  const engaged = () => pZoom !== null || rec.queued.length > 0;

  // --- derived view state -------------------------------------------------

  /** Zoomed and actually showing full-res pixels. */
  const isApplied = computed(() => {
    void recordVersion.value;
    return zoom.value !== null && rec.layout !== null && rec.decoded;
  });

  /**
   * Zoomed styles for the full-res <img>. While zoomed the image is laid out
   * at its natural size (one image pixel per device pixel) and positioned
   * with translate3d + scale(zoom). Scaling the fitted box up instead looks
   * equivalent, but WebKit then resamples the image even when the numbers
   * work out to exactly 1:1, softening every pixel — so at 100% the scale
   * here is exactly 1. Entering or leaving zoom is one layout change; pan
   * and zoom after that only touch the transform.
   */
  const zoomedStyle = computed<CSSProperties | undefined>(() => {
    void recordVersion.value;
    const z = zoom.value;
    const l = rec.layout;
    if (z === null || !l || !rec.decoded) return undefined;
    const s = spans(l, z, centerX.value, centerY.value);
    // Keep the image on whole device pixels; a sub-pixel offset would
    // resample every pixel too. The stage itself can sit on a fractional
    // device pixel (e.g. y = 79.5 at 1×), and WebKit snaps a composited
    // layer's layout position separately from its transform, so: pull the
    // box back onto the grid with left/top, then translate by whole device
    // pixels from there.
    const d = l.dpr;
    const phaseX = Math.floor(l.ox * d) / d - l.ox;
    const phaseY = Math.floor(l.oy * d) / d - l.oy;
    const tx = (Math.round((l.ox + s.sx) * d) - Math.floor(l.ox * d)) / d;
    const ty = (Math.round((l.oy + s.sy) * d) - Math.floor(l.oy * d)) / d;
    return {
      inset: "auto",
      left: `${phaseX}px`,
      top: `${phaseY}px`,
      margin: "0",
      maxWidth: "none",
      maxHeight: "none",
      width: `${l.natW / d}px`,
      height: `${l.natH / d}px`,
      transform: `translate3d(${tx}px, ${ty}px, 0) scale(${z})`,
      transformOrigin: "0 0",
      willChange: "transform",
    };
  });

  const imageStyle = computed<CSSProperties>(
    () => zoomedStyle.value ?? { cursor: enabled.value ? "zoom-in" : undefined }
  );

  /** Zoom requested (or in effect), possibly still waiting for full-res. */
  const isZoomed = computed(() => zoom.value !== null || queuedKind.value !== null);

  /** For the thumbnail placeholder: it can be clicked to request a zoom. */
  const placeholderStyle = computed<CSSProperties>(() => ({
    cursor: enabled.value && !isZoomed.value ? "zoom-in" : undefined,
  }));

  const stageStyle = computed<CSSProperties>(() =>
    isApplied.value ? { cursor: dragging.value ? "grabbing" : "grab" } : {}
  );

  /** Zoom requested, full-res image not loaded/decoded yet. */
  const loading = computed(() => isZoomed.value && !isApplied.value);

  /** Zoom level for the indicator, e.g. 100; null at fit, and while queued
   *  wheel/pinch input waits for the image (its level depends on the
   *  image's size, so there is no honest number to show yet). */
  const percent = computed(() => {
    if (queuedKind.value === "scale") return null;
    if (queuedKind.value === "toggle") return TOGGLE_ZOOM * 100;
    return zoom.value === null ? null : Math.round(zoom.value * 100);
  });

  // --- commands -----------------------------------------------------------

  function reset() {
    endDrag();
    setFit();
    frame.flush();
  }

  /** Fit → toggle zoom at (fx, fy) (fractions of the stage); zoomed → fit. */
  function toggleAt(fx: number, fy: number) {
    if (!enabled.value) return;
    if (engaged()) {
      reset();
      return;
    }
    input({ kind: "toggle", fx, fy });
    frame.flush();
  }

  const handle: LoupeHandle = {
    toggle: () => toggleAt(0.5, 0.5),
    reset,
    isZoomed: () => enabled.value && engaged(),
  };

  // --- input --------------------------------------------------------------

  function stagePoint(e: { clientX: number; clientY: number }) {
    const rect = stage.value!.getBoundingClientRect();
    return {
      fx: (e.clientX - rect.left) / rect.width,
      fy: (e.clientY - rect.top) / rect.height,
    };
  }

  /** Clicks on the PTP download card and its button are not photo clicks. */
  function isOverlay(target: EventTarget | null) {
    return target instanceof Element && !!target.closest("button, .ptp-download");
  }

  // Safari/WKWebView reports trackpad pinch as GestureEvents; other engines
  // (and possibly some WebKit builds) send ctrl+wheel. Handle both, and while
  // a gesture is live ignore ctrl+wheel so a pinch can't apply twice.
  // Gestures are applied as incremental ratios (scale / previous scale), so
  // a pinch keeps one consistent baseline even if the photo finishes loading
  // mid-gesture.
  let gestureActive = false;
  let gestureLastScale = 1;

  function onWheel(e: WheelEvent) {
    if (!enabled.value || !stage.value) return;
    e.preventDefault();
    if (e.ctrlKey && gestureActive) return;
    // deltaMode 1 = lines (some mice): roughly 16 px per line.
    const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    const factor = wheelZoomFactor(
      dy,
      e.ctrlKey ? PINCH_SENSITIVITY : undefined
    );
    input({ kind: "scale", factor, ...stagePoint(e) });
    frame.schedule();
  }

  function onGestureStart(e: Event) {
    if (!enabled.value) return;
    e.preventDefault();
    gestureActive = true;
    gestureLastScale = 1;
  }

  function onGestureChange(e: Event) {
    if (!enabled.value || !stage.value) return;
    e.preventDefault();
    const g = e as GestureLikeEvent;
    if (!(g.scale > 0)) return;
    const factor = g.scale / gestureLastScale;
    gestureLastScale = g.scale;
    input({ kind: "scale", factor, ...stagePoint(g) });
    frame.schedule();
  }

  function onGestureEnd(e: Event) {
    e.preventDefault();
    gestureActive = false;
  }

  // --- press / drag -------------------------------------------------------
  //
  // Pointer events with pointer capture once a pan starts, so the drag
  // follows the pointer outside the window. A drag ends on pointerup, on
  // pointercancel / lostpointercapture, when the window loses focus (app
  // switch mid-drag), or on any move that arrives without the primary
  // button held — never left "stuck" waiting for a mouseup that won't come.
  //
  // Capture is taken only once the press turns into a pan, so a plain click
  // keeps its real target (the click handler needs to know whether it
  // landed on the photo or the letterbox).
  //
  // Ctrl+primary is a context click on macOS: it never pans or toggles.

  let suppressClick = false;
  let endDrag: () => void = () => {};

  function onMouseDown(e: MouseEvent) {
    // Stop WebKit's native image drag and text selection; the press itself
    // is handled through pointer events. Leave context clicks alone.
    if (enabled.value && e.button === 0 && !e.ctrlKey && !isOverlay(e.target)) {
      e.preventDefault();
    }
  }

  function onPointerDown(e: PointerEvent) {
    if (!enabled.value || e.button !== 0 || e.ctrlKey || !e.isPrimary || isOverlay(e.target)) {
      return;
    }
    const el = stage.value;
    if (!el) return;
    endDrag();
    suppressClick = false;

    const l = rec.layout;
    const panning = l !== null && pZoom !== null;
    const start = panning ? spans(l, pZoom!, pCx, pCy) : null;
    const pointerId = e.pointerId;
    const downX = e.clientX;
    const downY = e.clientY;
    let moved = false;
    let captured = false;

    function onMove(ev: PointerEvent) {
      if (ev.pointerId !== pointerId) return;
      if ((ev.buttons & 1) === 0) {
        // Button released somewhere we never heard about.
        finish();
        return;
      }
      const dx = ev.clientX - downX;
      const dy = ev.clientY - downY;
      if (!moved && Math.hypot(dx, dy) > DRAG_THRESHOLD) {
        moved = true;
        if (panning) {
          dragging.value = true;
          try {
            el!.setPointerCapture(pointerId);
            captured = true;
          } catch {
            // Pointer already gone; the window listeners still cover it.
          }
        }
      }
      if (panning && start && rec.layout === l && pZoom !== null) {
        setFromSpans(l!, pZoom, start.sx + dx, start.sy + dy);
        frame.schedule();
      }
    }

    function onUp(ev: PointerEvent) {
      if (ev.pointerId !== pointerId) return;
      finish();
    }

    function onCancel(ev: Event) {
      if (ev instanceof PointerEvent && ev.pointerId !== pointerId) return;
      finish();
    }

    function onBlur() {
      finish();
    }

    function finish() {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("blur", onBlur);
      el!.removeEventListener("lostpointercapture", onCancel);
      if (captured && el!.hasPointerCapture?.(pointerId)) {
        el!.releasePointerCapture(pointerId);
      }
      dragging.value = false;
      // A press that turned into a drag must not end in a click (toggle),
      // even when the drag was cut short and the button comes up later over
      // the stage. The next press clears this, so nothing stays armed.
      suppressClick = moved;
      endDrag = () => {};
    }

    endDrag = () => finish();
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("blur", onBlur);
    el.addEventListener("lostpointercapture", onCancel);
  }

  // Double-clicks are counted from accepted clicks only: `detail` counts
  // every press in the run, including ones that were drags (suppressed).
  // A click is the second of a double-click when the click before it in the
  // same run (detail - 1) was itself accepted.
  let lastAcceptedDetail = 0;

  function onClick(e: MouseEvent) {
    if (!enabled.value || isOverlay(e.target)) return;
    if (e.ctrlKey) return; // context click
    if (suppressClick) {
      suppressClick = false;
      lastAcceptedDetail = 0;
      return;
    }
    const isDouble = e.detail >= 2 && lastAcceptedDetail === e.detail - 1;
    // At fit only a click on the photo zooms (not the letterbox around it);
    // zoomed, a click anywhere returns to fit.
    if (!isDouble && !engaged() && !(e.target instanceof HTMLImageElement)) {
      lastAcceptedDetail = 0;
      return;
    }
    lastAcceptedDetail = e.detail;
    // Second click of a double-click: always land on fit, whatever the
    // first click did, so double-click never leaves the photo zoomed.
    if (isDouble) {
      reset();
      return;
    }
    const p = stagePoint(e);
    toggleAt(p.fx, p.fy);
  }

  // --- wiring -------------------------------------------------------------

  // Bound imperatively: wheel needs { passive: false } to preventDefault,
  // and the gesture events are WebKit-only (unknown to Vue's template types).
  const listeners: [string, EventListener, AddEventListenerOptions?][] = [
    ["wheel", onWheel as EventListener, { passive: false }],
    ["gesturestart", onGestureStart],
    ["gesturechange", onGestureChange],
    ["gestureend", onGestureEnd],
    ["pointerdown", onPointerDown as EventListener],
    ["mousedown", onMouseDown as EventListener],
    // No dblclick listener: onClick handles the second click itself, and a
    // dblclick also fires when the second press was a drag, which must not
    // unzoom.
    ["click", onClick as EventListener],
  ];

  const resizeObserver =
    typeof ResizeObserver !== "undefined" ? new ResizeObserver(() => measure()) : null;

  watch(
    stage,
    (el, prev) => {
      for (const [type, fn, opts] of listeners) {
        prev?.removeEventListener(type, fn, opts);
        el?.addEventListener(type, fn, opts);
      }
      resizeObserver?.disconnect();
      if (el) resizeObserver?.observe(el);
    },
    { flush: "post" }
  );

  // A new photo gets a fresh record: its predecessor's layout, decode state
  // and queued input go with it. The view zoom is kept (see the header).
  watch(
    photoId,
    (id) => {
      if (id === rec.id) return;
      clearDwell();
      rec = newRecord(id);
      frame.flush();
      recordVersion.value++;
    },
    { flush: "sync" }
  );

  // Attach the current photo's <img> and measure once it has loaded. A
  // different element for the same photo (e.g. after a PTP download) keeps
  // the record's queued input but resets what was measured for the old one.
  watch(
    [image, ready],
    () => {
      const img = image.value;
      if (img !== rec.el) {
        rec.el = img;
        rec.layout = null;
        rec.decoded = false;
        rec.decodeRequested = false;
        clearDwell();
      }
      measure();
      if (rec.layout) requestDecode(engaged());
    },
    { flush: "post" }
  );

  // The stage can move without resizing (it is re-laid out with the window),
  // which shifts its device-pixel phase; re-measure on window resize too.
  function onWindowResize() {
    if (rec.layout) measure();
  }

  // devicePixelRatio changes (window dragged to another display) don't
  // resize the stage, so watch the resolution media query as well.
  let dprQuery: MediaQueryList | null = null;
  function watchDpr() {
    dprQuery?.removeEventListener("change", onDprChange);
    dprQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
    dprQuery.addEventListener("change", onDprChange);
  }
  function onDprChange() {
    watchDpr();
    measure();
  }

  // Videos can't be zoomed; don't carry an invisible zoom across one.
  watch(enabled, (on) => {
    if (!on) reset();
  });

  onMounted(() => {
    watchDpr();
    window.addEventListener("resize", onWindowResize);
    registerLoupe(handle);
  });

  onScopeDispose(() => {
    frame.cancel();
    endDrag();
    clearDwell();
    // Any decode still queued for this viewer is skipped: the record is
    // no longer current once replaced.
    rec = newRecord(undefined);
    resizeObserver?.disconnect();
    dprQuery?.removeEventListener("change", onDprChange);
    window.removeEventListener("resize", onWindowResize);
    const el = stage.value;
    if (el) for (const [type, fn, opts] of listeners) el.removeEventListener(type, fn, opts);
    unregisterLoupe(handle);
  });

  return {
    isZoomed,
    isApplied,
    loading,
    percent,
    imageStyle,
    placeholderStyle,
    stageStyle,
    reset,
  };
}
