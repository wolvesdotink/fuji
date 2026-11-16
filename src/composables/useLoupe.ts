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
 * Zoom state is stored independently of the current photo's size:
 *   - `zoom` — device pixels per image pixel (1 = 100%, one image pixel per
 *     screen pixel on any display), or null for "fit".
 *   - `centerX`, `centerY` — the image point at the centre of the view, as a
 *     fraction of the image (0..1).
 * The CSS transform is derived from that plus the current layout, so the
 * same state survives a window resize, a move to a display with a different
 * devicePixelRatio, and navigation to a neighbour of a different size or
 * orientation (zoom level and relative position are kept, Lightroom-style).
 *
 * At fit the `<img>` keeps its normal fitted layout and gets no inline
 * styles at all, so the fitted view renders exactly as before. Zoomed, it is
 * laid out once at its natural size and moved with `translate3d() scale()`
 * (see `zoomedStyle`), so panning and zooming never trigger layout.
 *
 * The 600 px thumbnail placeholder is never zoomed: until the full-res image
 * has loaded (`ready`), zoom input (click, Z, wheel, pinch) is remembered —
 * the requested zoom and where it was aimed — and applied on load, while the
 * view shows the fitted placeholder and the indicator says the
 * full-resolution image is still loading.
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

export interface LoupeOptions {
  /** The element zoom input is bound to (the viewer's image container). */
  stage: Ref<HTMLElement | null>;
  /** The full-resolution `<img>`. */
  image: Ref<HTMLImageElement | null>;
  /** True once the full-resolution image has loaded. */
  ready: Ref<boolean>;
  /** False for videos: the loupe resets and ignores all input. */
  enabled: Ref<boolean>;
}

/**
 * Zoom input that arrived before the full-res image (and so its size) was
 * known. `toggle`: click / Z, i.e. "100% here"; `factor`: wheel / pinch,
 * relative to whatever zoom was in effect. `factor` accumulates further
 * wheel/pinch input; (fx, fy) is the latest anchor as a fraction of the stage.
 */
interface PendingZoom {
  kind: "toggle" | "factor";
  factor: number;
  fx: number;
  fy: number;
}

interface GestureLikeEvent extends UIEvent {
  scale: number;
  clientX: number;
  clientY: number;
}

export function useLoupe({ stage, image, ready, enabled }: LoupeOptions) {
  // Committed (reactive) state — drives the transform and the indicator.
  const zoom = ref<number | null>(null);
  const centerX = ref(0.5);
  const centerY = ref(0.5);
  const pendingKind = ref<PendingZoom["kind"] | null>(null);
  const dragging = ref(false);
  const layoutVersion = ref(0);
  // Full-resolution decode of the current image has finished (see the
  // ready/image watcher).
  const decoded = ref(false);

  // Pending state, written synchronously by input handlers and committed
  // once per frame (see createFrameCommit).
  let pZoom: number | null = null;
  let pCx = 0.5;
  let pCy = 0.5;
  // Zoom input received before the full-res image loaded, applied by
  // measure() once the natural size is known.
  let pending: PendingZoom | null = null;

  let layout: Layout | null = null;

  const frame = createFrameCommit(() => {
    zoom.value = pZoom;
    centerX.value = pCx;
    centerY.value = pCy;
    pendingKind.value = pending?.kind ?? null;
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
    pending = null;
  }

  /** Zoom to `target`, keeping the image point under (ax, ay) in place. */
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

  /** Re-validate a kept zoom against the current image and viewport. */
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

  function measure() {
    const img = image.value;
    const el = stage.value;
    if (!ready.value || !img || !el || !img.naturalWidth || !img.naturalHeight) {
      layout = null;
      layoutVersion.value++;
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
    layout = l;

    const req = pending;
    if (req) {
      // Establish the base the input was relative to, then apply it.
      pending = null;
      let target: number;
      if (req.kind === "toggle") {
        setFit();
        target = toggleZoom(l) * req.factor;
      } else {
        revalidate(l);
        target = (pZoom ?? fitZoom(l)) * req.factor;
      }
      zoomAt(l, target, req.fx * l.vw, req.fy * l.vh);
    } else {
      revalidate(l);
    }
    frame.flush();
    layoutVersion.value++;
  }

  // --- derived view state -------------------------------------------------

  /** Zoomed and actually showing full-res pixels. */
  const isApplied = computed(() => {
    void layoutVersion.value;
    return zoom.value !== null && ready.value && decoded.value && layout !== null;
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
    void layoutVersion.value;
    const z = zoom.value;
    const l = layout;
    if (z === null || !l || !ready.value || !decoded.value) return undefined;
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
  const isZoomed = computed(() => zoom.value !== null || pendingKind.value !== null);

  /** For the thumbnail placeholder: it can be clicked to request a zoom. */
  const placeholderStyle = computed<CSSProperties>(() => ({
    cursor: enabled.value && !isZoomed.value ? "zoom-in" : undefined,
  }));

  const stageStyle = computed<CSSProperties>(() =>
    isApplied.value ? { cursor: dragging.value ? "grabbing" : "grab" } : {}
  );

  /** Zoom requested, full-res image not loaded yet. */
  const loading = computed(() => isZoomed.value && !isApplied.value);

  /** Zoom level for the indicator, e.g. 100; null at fit, and while a
   *  wheel/pinch zoom waits for the image (its level depends on the image's
   *  size, so there is no honest number to show yet). */
  const percent = computed(() => {
    if (pendingKind.value === "factor") return null;
    if (pendingKind.value === "toggle") return TOGGLE_ZOOM * 100;
    return zoom.value === null ? null : Math.round(zoom.value * 100);
  });

  // --- commands -----------------------------------------------------------

  const engaged = () => pZoom !== null || pending !== null;

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
    if (layout) {
      zoomAt(layout, toggleZoom(layout), fx * layout.vw, fy * layout.vh);
    } else {
      // Not loaded yet: remember the request. The exact target depends on
      // the natural size, so measure() resolves it on load.
      pending = { kind: "toggle", factor: 1, fx, fy };
    }
    frame.flush();
  }

  /** Wheel/pinch before load: accumulate the factor, keep the last anchor. */
  function requestFactor(factor: number, fx: number, fy: number) {
    const total = (pending?.factor ?? 1) * factor;
    if (pending?.kind === "toggle") {
      pending = { kind: "toggle", factor: total, fx, fy };
    } else if (pZoom === null && total <= 1 + FIT_EPSILON) {
      // Zoomed out at fit: nothing to apply.
      pending = null;
    } else {
      pending = { kind: "factor", factor: total, fx, fy };
    }
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

  // Safari/WKWebView reports trackpad pinch as GestureEvents; other engines
  // (and possibly some WebKit builds) send ctrl+wheel. Handle both, and while
  // a gesture is live ignore ctrl+wheel so a pinch can't apply twice.
  let gestureActive = false;
  let gestureStartZoom = 1;
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
    const p = stagePoint(e);
    const l = layout;
    if (l) zoomAt(l, (pZoom ?? fitZoom(l)) * factor, p.x, p.y);
    else requestFactor(factor, p.fx, p.fy);
    frame.schedule();
  }

  function onGestureStart(e: Event) {
    if (!enabled.value) return;
    e.preventDefault();
    gestureActive = true;
    gestureLastScale = 1;
    gestureStartZoom = layout ? (pZoom ?? fitZoom(layout)) : 1;
  }

  function onGestureChange(e: Event) {
    if (!enabled.value || !stage.value) return;
    e.preventDefault();
    const g = e as GestureLikeEvent;
    const p = stagePoint(g);
    const l = layout;
    if (l) {
      zoomAt(l, gestureStartZoom * g.scale, p.x, p.y);
    } else {
      // `scale` is cumulative for the gesture; feed only the change.
      requestFactor(g.scale / gestureLastScale, p.fx, p.fy);
    }
    gestureLastScale = g.scale;
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

  let suppressClick = false;
  let endDrag: () => void = () => {};

  function onMouseDown(e: MouseEvent) {
    // Stop WebKit's native image drag and text selection; the press itself
    // is handled through pointer events.
    if (enabled.value && e.button === 0 && !isOverlay(e.target)) e.preventDefault();
  }

  function onPointerDown(e: PointerEvent) {
    if (!enabled.value || e.button !== 0 || !e.isPrimary || isOverlay(e.target)) return;
    const el = stage.value;
    if (!el) return;
    endDrag();
    suppressClick = false;

    const l = layout;
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
      if (panning && start && layout && pZoom !== null) {
        setFromSpans(layout, pZoom, start.sx + dx, start.sy + dy);
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

  function onClick(e: MouseEvent) {
    if (!enabled.value || isOverlay(e.target)) return;
    if (suppressClick) {
      suppressClick = false;
      return;
    }
    // Second click of a double-click: always land on fit, whatever the
    // first click did, so double-click never leaves the photo zoomed.
    if (e.detail >= 2) {
      reset();
      return;
    }
    // At fit only a click on the photo zooms (not the letterbox around it);
    // zoomed, a click anywhere returns to fit.
    if (!engaged() && !(e.target instanceof HTMLImageElement)) return;
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
    // No dblclick listener: onClick handles the second click via `detail`,
    // and a dblclick also fires when the second press was a drag, which
    // must not unzoom.
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

  // The stage can move without resizing (it is re-laid out with the window),
  // which shifts its device-pixel phase; re-measure on window resize too.
  function onWindowResize() {
    if (layout) measure();
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

  // (Re)measure once the full-res image is in the DOM and loaded. Leaving it
  // (navigation) drops the layout, so the kept zoom waits for the next load.
  //
  // Then decode it at full resolution, off the main thread. WebKit decodes a
  // large image only at the size it is displayed, so the fitted view holds a
  // subsampled bitmap; laying the image out at natural size for the first
  // zoom would otherwise force a synchronous full decode on the next paint
  // (~300 ms for 26 MP in testing). Zoom is applied once this finishes;
  // until then it is shown as loading, like a zoom requested before load.
  let decodeToken = 0;
  watch(
    [ready, image],
    () => {
      const token = ++decodeToken;
      decoded.value = false;
      measure();
      const img = image.value;
      if (!ready.value || !img) return;
      img
        .decode()
        .catch(() => {})
        .finally(() => {
          if (token === decodeToken) decoded.value = true;
        });
    },
    { flush: "post" }
  );

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
