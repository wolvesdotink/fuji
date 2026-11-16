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
 *   is, its measured layout, whether its full-resolution decode is done,
 *   whether it failed to load, and a zoom requested before it could be
 *   zoomed. The record is replaced wholesale when the photo changes, so
 *   nothing from one photo (a late load event, a pending request, a
 *   finishing decode) can land on the next.
 *
 * At fit the `<img>` keeps its normal fitted layout and gets no inline
 * styles at all, so the fitted view renders exactly as before. Zoomed, it is
 * laid out once at its natural size and moved with `translate3d() scale()`
 * (see `zoomedStyle`), so panning and zooming never trigger layout.
 *
 * Before the full-resolution image has loaded its size is unknown, so zoom
 * input can't be applied exactly. One simple rule instead: any zoom-in
 * intent (click, Z, wheel in, pinch out) sets a single pending request, "go
 * to 100% here" (the latest anchor wins; Z aims at the centre), shown with
 * the loading indicator; any zoom-out intent, Esc, or leaving the photo
 * clears it. When the image is ready the request is applied once. The 600 px
 * thumbnail placeholder is never zoomed.
 */

/** Highest zoom: one image pixel spread over 4×4 device pixels (400%). */
const MAX_ZOOM = 4;
/** Click / Z target: one image pixel per device pixel. */
const TOGGLE_ZOOM = 1;
/** Trackpad pinch reports small deltas; scale them up to feel direct. */
const PINCH_SENSITIVITY = 0.01;
/** Pointer travel (CSS px) after which a press counts as a drag, not a click. */
const DRAG_THRESHOLD = 3;
/**
 * How long a photo must stay on screen before its full-resolution decode
 * starts (unless zoom is already engaged). Arrowing through a card doesn't
 * start a 26 MP decode per photo.
 */
const DECODE_DWELL_MS = 300;
/** Full-resolution decodes in flight at once: the current photo's, plus at
 *  most one for a photo already left (decode() can't be cancelled). */
const MAX_DECODES = 2;

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
  /** The full-res image failed to load: nothing to wait for. */
  failed: boolean;
  /** "Go to 100% here", requested before the image could be zoomed;
   *  (fx, fy) is the anchor as a fraction of the stage. */
  pending: { fx: number; fy: number } | null;
}

const newRecord = (id: string | undefined): PhotoRecord => ({
  id,
  el: null,
  layout: null,
  decoded: false,
  decodeRequested: false,
  failed: false,
  pending: null,
});

// Full-resolution decodes, app-wide. The current photo's decode starts at
// once unless MAX_DECODES are already running (then it takes the next free
// slot; only the latest waiting request is kept). Results for a photo that
// is no longer current are ignored by the caller.
let decodesInFlight = 0;
let waitingDecode: (() => void) | null = null;

function scheduleDecode(job: () => Promise<void>) {
  const run = () => {
    decodesInFlight++;
    job().finally(() => {
      decodesInFlight--;
      const next = waitingDecode;
      waitingDecode = null;
      next?.();
    });
  };
  if (decodesInFlight < MAX_DECODES) run();
  else waitingDecode = run;
}

export interface LoupeOptions {
  /** The element zoom input is bound to (the viewer's image container). */
  stage: Ref<HTMLElement | null>;
  /** Identity of the photo on screen. */
  photoId: Ref<string | undefined>;
  /** The full-resolution `<img>`. */
  image: Ref<HTMLImageElement | null>;
  /** True once the current full-resolution image has loaded. */
  ready: Ref<boolean>;
  /** True if the current full-resolution image failed to load. */
  failed: Ref<boolean>;
  /** False for videos: the loupe resets and ignores all input. */
  enabled: Ref<boolean>;
}

interface GestureLikeEvent extends UIEvent {
  scale: number;
  clientX: number;
  clientY: number;
}

export function useLoupe({ stage, photoId, image, ready, failed, enabled }: LoupeOptions) {
  // Committed (reactive) view state — drives the transform and indicator.
  const zoom = ref<number | null>(null);
  const centerX = ref(0.5);
  const centerY = ref(0.5);
  const hasPending = ref(false);
  const dragging = ref(false);
  /** Bumped whenever the current record's layout, decode or load state
   *  changes. */
  const recordVersion = ref(0);

  // Pending view state, written synchronously by input handlers and
  // committed once per frame (see createFrameCommit).
  let pZoom: number | null = null;
  let pCx = 0.5;
  let pCy = 0.5;

  let rec: PhotoRecord = newRecord(photoId.value);
  /** Trackpad pinch in progress (see onGestureChange). */
  let gesture: { refZoom: number | null; refScale: number; lastScale: number } | null = null;

  const frame = createFrameCommit(() => {
    zoom.value = pZoom;
    centerX.value = pCx;
    centerY.value = pCy;
    hasPending.value = rec.pending !== null;
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
    rec.pending = null;
  }

  /**
   * Zoom to `target`, keeping the image point under (ax, ay) in place.
   * "At fit" is a clamp on the result: only a target at or below fit lands
   * on fit; any target above it, however slightly, is kept, so slow wheel or
   * pinch movement near fit is never thrown away.
   */
  function zoomAt(l: Layout, target: number, ax: number, ay: number) {
    const z = pZoom ?? fitZoom(l);
    const next = clamp(target, fitZoom(l), maxZoom(l));
    if (next <= fitZoom(l)) {
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
    if (z <= fitZoom(l)) {
      setFit();
    } else {
      const s = spans(l, z, pCx, pCy);
      setFromSpans(l, z, s.sx, s.sy);
    }
  }

  /**
   * Zoom intent before the image can be zoomed. In: "100% here" (latest
   * anchor wins) — unless a zoom carried over from the previous photo is
   * already waiting to apply, which it then keeps. Out: back to fit,
   * dropping the request (and a carried zoom).
   */
  function preloadIntent(zoomIn: boolean, fx: number, fy: number) {
    if (zoomIn) {
      if (pZoom === null) rec.pending = { fx, fy };
    } else {
      setFit();
    }
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
    const firstLayout = rec.layout === null;
    rec.layout = l;

    revalidate(l);
    // Apply the pending "100% here" request once.
    const req = rec.pending;
    if (req) {
      rec.pending = null;
      pZoom = null;
      zoomAt(l, toggleZoom(l), req.fx * l.vw, req.fy * l.vh);
    }
    // A pinch that was in progress continues from here.
    if (firstLayout && gesture) rebaseGesture(l);

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
  // once when zoom is engaged. Zoom is shown once its photo's decode is
  // done; until then it reads as loading.

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
    scheduleDecode(async () => {
      // Left this photo while waiting for a slot: skip the work entirely.
      if (r !== rec || r.el !== img) return;
      await img.decode().catch(() => {});
      // Superseded while decoding: ignore the result.
      if (r !== rec || r.el !== img) return;
      r.decoded = true;
      // Re-measure: layout may have changed while decoding, and this is the
      // moment a waiting zoom becomes visible.
      measure();
    });
  }

  const engaged = () => pZoom !== null || rec.pending !== null;

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
  const isZoomed = computed(() => zoom.value !== null || hasPending.value);

  /** For the thumbnail placeholder: it can be clicked to request a zoom. */
  const placeholderStyle = computed<CSSProperties>(() => ({
    cursor: enabled.value && !isZoomed.value ? "zoom-in" : undefined,
  }));

  const stageStyle = computed<CSSProperties>(() =>
    isApplied.value ? { cursor: dragging.value ? "grabbing" : "grab" } : {}
  );

  /** This photo's full-res image failed: there is nothing to wait for. */
  const photoFailed = computed(() => {
    void recordVersion.value;
    return rec.failed;
  });

  /** Zoom requested, full-res image not loaded/decoded yet. */
  const loading = computed(
    () => isZoomed.value && !isApplied.value && !photoFailed.value
  );

  /** Zoom level for the indicator, e.g. 100; null at fit, and when this
   *  photo's full-res image failed to load. */
  const percent = computed(() => {
    if (photoFailed.value) return null;
    if (hasPending.value) return TOGGLE_ZOOM * 100;
    return zoom.value === null ? null : Math.round(zoom.value * 100);
  });

  // --- commands -----------------------------------------------------------

  function reset() {
    endDrag();
    setFit();
    frame.flush();
  }

  /** Fit → 100% at (fx, fy) (fractions of the stage); zoomed → fit. */
  function toggleAt(fx: number, fy: number) {
    if (!enabled.value) return;
    if (engaged()) {
      reset();
      return;
    }
    const l = rec.layout;
    if (l) zoomAt(l, toggleZoom(l), fx * l.vw, fy * l.vh);
    else preloadIntent(true, fx, fy);
    requestDecode(true);
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
      x: e.clientX - rect.left,
      y: e.clientY - rect.top,
      fx: (e.clientX - rect.left) / rect.width,
      fy: (e.clientY - rect.top) / rect.height,
    };
  }

  /** Clicks on the PTP download card and its button are not photo clicks. */
  function isOverlay(target: EventTarget | null) {
    return target instanceof Element && !!target.closest("button, .ptp-download");
  }

  function onWheel(e: WheelEvent) {
    if (!enabled.value || !stage.value) return;
    e.preventDefault();
    if (e.ctrlKey && gesture) return;
    // deltaMode 1 = lines (some mice): roughly 16 px per line.
    const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    if (dy === 0) return;
    const factor = wheelZoomFactor(
      dy,
      e.ctrlKey ? PINCH_SENSITIVITY : undefined
    );
    const p = stagePoint(e);
    const l = rec.layout;
    if (l) zoomAt(l, (pZoom ?? fitZoom(l)) * factor, p.x, p.y);
    else preloadIntent(factor > 1, p.fx, p.fy);
    if (engaged()) requestDecode(true);
    frame.schedule();
  }

  // Trackpad pinch. Safari/WKWebView reports it as GestureEvents; other
  // engines (and possibly some WebKit builds) send ctrl+wheel. Handle both,
  // and while a gesture is live ignore ctrl+wheel so a pinch can't apply
  // twice.
  //
  // A gesture's zoom is derived from a reference — the zoom and the gesture
  // scale at its start, or at the last re-base — so it is exact however
  // slowly the fingers move: zoom = refZoom × scale / refScale, clamped.
  // The reference is re-based when the photo becomes zoomable mid-gesture
  // (at the zoom just applied and the gesture's current scale).

  function rebaseGesture(l: Layout) {
    if (!gesture) return;
    gesture.refZoom = pZoom ?? fitZoom(l);
    gesture.refScale = gesture.lastScale;
  }

  function onGestureStart(e: Event) {
    if (!enabled.value) return;
    e.preventDefault();
    const l = rec.layout;
    gesture = { refZoom: l ? (pZoom ?? fitZoom(l)) : null, refScale: 1, lastScale: 1 };
  }

  function onGestureChange(e: Event) {
    if (!enabled.value || !stage.value || !gesture) return;
    e.preventDefault();
    const g = e as GestureLikeEvent;
    if (!(g.scale > 0)) return;
    const p = stagePoint(g);
    const l = rec.layout;
    if (l && gesture.refZoom !== null) {
      zoomAt(l, (gesture.refZoom * g.scale) / gesture.refScale, p.x, p.y);
    } else if (g.scale !== gesture.lastScale) {
      preloadIntent(g.scale > gesture.lastScale, p.fx, p.fy);
    }
    gesture.lastScale = g.scale;
    if (engaged()) requestDecode(true);
    frame.schedule();
  }

  function onGestureEnd(e: Event) {
    e.preventDefault();
    gesture = null;
  }

  // --- press / drag -------------------------------------------------------
  //
  // Pointer events with pointer capture once a pan starts, so the drag
  // follows the pointer outside the window. A drag ends on pointerup, on
  // pointercancel / lostpointercapture, when the window loses focus (app
  // switch mid-drag), on any move that arrives without the primary button
  // held, or when the photo changes — never left "stuck" waiting for a
  // mouseup that won't come.
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
  // and pending request go with it, and so does any drag in progress. The
  // view zoom is kept (see the header).
  watch(
    photoId,
    (id) => {
      if (id === rec.id) return;
      endDrag();
      clearDwell();
      rec = newRecord(id);
      frame.flush();
      recordVersion.value++;
    },
    { flush: "sync" }
  );

  // Attach the current photo's <img> and measure once it has loaded. A
  // different element for the same photo (e.g. after a PTP download) keeps
  // the record's pending request but resets what was measured for the old
  // one.
  watch(
    [image, ready],
    () => {
      const img = image.value;
      if (img !== rec.el) {
        rec.el = img;
        rec.layout = null;
        rec.decoded = false;
        rec.decodeRequested = false;
        rec.failed = false;
        clearDwell();
      }
      measure();
      if (rec.layout) requestDecode(engaged());
    },
    { flush: "post" }
  );

  // The current full-res image failed to load: drop the pending request
  // and stop showing a loading state that can't end.
  watch(failed, (f) => {
    if (!f) return;
    rec.failed = true;
    rec.pending = null;
    frame.flush();
    recordVersion.value++;
  });

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
    // Any decode still waiting for this viewer is skipped: the record is
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
