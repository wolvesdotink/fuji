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
 * The `<img>` keeps its normal fitted layout (max-width/max-height: 100%,
 * centred); zoom is only a `translate3d() scale()` on top of it, so zooming
 * and panning never trigger layout. At fit there is no transform at all, so
 * the fitted view renders exactly as it did before the loupe existed.
 *
 * The 600 px thumbnail placeholder is never zoomed: until the full-res image
 * has loaded (`ready`), a zoom request is remembered and applied on load,
 * while the view shows the fitted placeholder and the indicator says the
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
  const dragging = ref(false);
  const layoutVersion = ref(0);

  // Pending state, written synchronously by input handlers and committed
  // once per frame (see createFrameCommit).
  let pZoom: number | null = null;
  let pCx = 0.5;
  let pCy = 0.5;
  // A zoom requested before the full-res image loaded: where it was aimed,
  // as a fraction of the stage, so it can be applied once the size is known.
  let pendingAnchor: { fx: number; fy: number } | null = null;

  let layout: Layout | null = null;

  const frame = createFrameCommit(() => {
    zoom.value = pZoom;
    centerX.value = pCx;
    centerY.value = pCy;
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
    pendingAnchor = null;
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
      natW,
      natH,
      fit: Math.min(1, rect.width / natW, rect.height / natH),
      dpr: window.devicePixelRatio || 1,
    };
    if (l.vw === 0 || l.vh === 0) return;
    layout = l;

    if (pendingAnchor) {
      const { fx, fy } = pendingAnchor;
      setFit();
      zoomAt(l, toggleZoom(l), fx * l.vw, fy * l.vh);
    } else if (pZoom !== null) {
      // Re-validate the kept zoom against this image and viewport.
      const z = clamp(pZoom, fitZoom(l), maxZoom(l));
      if (z <= fitZoom(l) * (1 + FIT_EPSILON)) {
        setFit();
      } else {
        const s = spans(l, z, pCx, pCy);
        setFromSpans(l, z, s.sx, s.sy);
      }
    }
    frame.flush();
    layoutVersion.value++;
  }

  // --- derived view state -------------------------------------------------

  /** Zoomed and actually showing full-res pixels. */
  const isApplied = computed(() => {
    void layoutVersion.value;
    return zoom.value !== null && ready.value && layout !== null;
  });

  const transform = computed<string | undefined>(() => {
    void layoutVersion.value;
    const z = zoom.value;
    const l = layout;
    if (z === null || !l || !ready.value) return undefined;
    const s = spans(l, z, centerX.value, centerY.value);
    // Snap to whole device pixels: at 100% a sub-pixel offset would resample
    // every pixel and soften exactly the detail the loupe is for.
    const snap = (v: number) => Math.round(v * l.dpr) / l.dpr;
    // The <img> is laid out fitted and centred; transform from that box.
    const boxX = (l.vw - l.natW * l.fit) / 2;
    const boxY = (l.vh - l.natH * l.fit) / 2;
    const scale = z / (l.dpr * l.fit);
    return `translate3d(${snap(s.sx) - boxX}px, ${snap(s.sy) - boxY}px, 0) scale(${scale})`;
  });

  const imageStyle = computed<CSSProperties>(() => {
    const t = transform.value;
    if (!t) return { cursor: enabled.value ? "zoom-in" : undefined };
    return { transform: t, transformOrigin: "0 0", willChange: "transform" };
  });

  /** For the thumbnail placeholder: it can be clicked to request a zoom. */
  const placeholderStyle = computed<CSSProperties>(() => ({
    cursor: enabled.value && zoom.value === null ? "zoom-in" : undefined,
  }));

  const stageStyle = computed<CSSProperties>(() =>
    isApplied.value ? { cursor: dragging.value ? "grabbing" : "grab" } : {}
  );

  /** Zoom level for the indicator, e.g. 100; null at fit. */
  const percent = computed(() =>
    zoom.value === null ? null : Math.round(zoom.value * 100)
  );
  /** Zoom requested, full-res image not loaded yet. */
  const loading = computed(() => zoom.value !== null && !isApplied.value);

  // --- commands -----------------------------------------------------------

  function reset() {
    setFit();
    frame.flush();
  }

  /** Fit → toggle zoom at (fx, fy) (fractions of the stage); zoomed → fit. */
  function toggleAt(fx: number, fy: number) {
    if (!enabled.value) return;
    if (pZoom !== null) {
      reset();
      return;
    }
    if (layout) {
      zoomAt(layout, toggleZoom(layout), fx * layout.vw, fy * layout.vh);
    } else {
      // Not loaded yet: remember the request. The exact target depends on
      // the natural size, so measure() recomputes it on load.
      pZoom = TOGGLE_ZOOM;
      pendingAnchor = { fx, fy };
    }
    frame.flush();
  }

  const handle: LoupeHandle = {
    toggle: () => toggleAt(0.5, 0.5),
    reset,
    isZoomed: () => enabled.value && pZoom !== null,
  };

  // --- input --------------------------------------------------------------

  function stagePoint(e: { clientX: number; clientY: number }) {
    const rect = stage.value!.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top, rect };
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

  function onWheel(e: WheelEvent) {
    if (!enabled.value || !stage.value) return;
    e.preventDefault();
    const l = layout;
    if (!l || (e.ctrlKey && gestureActive)) return;
    // deltaMode 1 = lines (some mice): roughly 16 px per line.
    const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    const factor = wheelZoomFactor(
      dy,
      e.ctrlKey ? PINCH_SENSITIVITY : undefined
    );
    const p = stagePoint(e);
    zoomAt(l, (pZoom ?? fitZoom(l)) * factor, p.x, p.y);
    frame.schedule();
  }

  function onGestureStart(e: Event) {
    if (!enabled.value) return;
    e.preventDefault();
    gestureActive = true;
    gestureStartZoom = layout ? (pZoom ?? fitZoom(layout)) : 1;
  }

  function onGestureChange(e: Event) {
    if (!enabled.value) return;
    e.preventDefault();
    const l = layout;
    if (!l || !stage.value) return;
    const g = e as GestureLikeEvent;
    const p = stagePoint(g);
    zoomAt(l, gestureStartZoom * g.scale, p.x, p.y);
    frame.schedule();
  }

  function onGestureEnd(e: Event) {
    e.preventDefault();
    gestureActive = false;
  }

  let suppressClick = false;
  let detachDrag: (() => void) | null = null;

  function onMouseDown(e: MouseEvent) {
    if (!enabled.value || e.button !== 0 || isOverlay(e.target)) return;
    // Stop WebKit's native image drag and text selection.
    e.preventDefault();
    suppressClick = false;

    const l = layout;
    const panning = l !== null && pZoom !== null;
    const start = panning ? spans(l, pZoom!, pCx, pCy) : null;
    const downX = e.clientX;
    const downY = e.clientY;
    let moved = false;

    function onMove(ev: MouseEvent) {
      const dx = ev.clientX - downX;
      const dy = ev.clientY - downY;
      if (!moved && Math.hypot(dx, dy) > DRAG_THRESHOLD) {
        moved = true;
        if (panning) dragging.value = true;
      }
      if (panning && start && layout && pZoom !== null) {
        setFromSpans(layout, pZoom, start.sx + dx, start.sy + dy);
        frame.schedule();
      }
    }

    function onUp() {
      detach();
      dragging.value = false;
      suppressClick = moved;
    }

    function detach() {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      detachDrag = null;
    }

    detachDrag?.();
    detachDrag = detach;
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
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
    if (pZoom === null && !(e.target instanceof HTMLImageElement)) return;
    const p = stagePoint(e);
    toggleAt(p.x / p.rect.width, p.y / p.rect.height);
  }

  function onDblClick() {
    if (enabled.value) reset();
  }

  // --- wiring -------------------------------------------------------------

  // Bound imperatively: wheel needs { passive: false } to preventDefault,
  // and the gesture events are WebKit-only (unknown to Vue's template types).
  const listeners: [string, EventListener, AddEventListenerOptions?][] = [
    ["wheel", onWheel as EventListener, { passive: false }],
    ["gesturestart", onGestureStart],
    ["gesturechange", onGestureChange],
    ["gestureend", onGestureEnd],
    ["mousedown", onMouseDown as EventListener],
    ["click", onClick as EventListener],
    ["dblclick", onDblClick],
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
  watch([ready, image], () => measure(), { flush: "post" });

  // Videos can't be zoomed; don't carry an invisible zoom across one.
  watch(enabled, (on) => {
    if (!on) reset();
  });

  onMounted(() => {
    watchDpr();
    registerLoupe(handle);
  });

  onScopeDispose(() => {
    frame.cancel();
    detachDrag?.();
    resizeObserver?.disconnect();
    dprQuery?.removeEventListener("change", onDprChange);
    const el = stage.value;
    if (el) for (const [type, fn, opts] of listeners) el.removeEventListener(type, fn, opts);
    unregisterLoupe(handle);
  });

  return {
    /** Zoom requested (possibly still waiting for full-res). */
    isZoomed: computed(() => zoom.value !== null),
    isApplied,
    loading,
    percent,
    imageStyle,
    placeholderStyle,
    stageStyle,
    reset,
  };
}
