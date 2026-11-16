/**
 * Zoom/pan math shared by the compare view (`useSyncedZoom`) and the
 * single-image loupe (`useLoupe`).
 *
 * Everything here works one axis at a time in a "content span" model: a
 * viewport of length `view`, and content (the photo) displayed with length
 * `len` starting at `start` (viewport coordinates, so `start` is usually ≤ 0
 * when zoomed in). Units are whatever the caller uses (normalized pane units
 * in compare, CSS pixels in the loupe); the math does not care.
 */

export function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, v));
}

/**
 * Wheel sensitivity: ~120 per wheel "click" on typical mice → a factor of
 * e^(-0.24) ≈ 0.79 per tick out. Exponential zoom feels more natural than
 * linear because each tick changes the scale by the same ratio.
 */
export const WHEEL_ZOOM_SENSITIVITY = 0.002;

/** Multiplicative zoom factor for one wheel event. */
export function wheelZoomFactor(
  deltaY: number,
  sensitivity: number = WHEEL_ZOOM_SENSITIVITY
): number {
  return Math.exp(-deltaY * sensitivity);
}

/**
 * Content start after scaling by `ratio` (newLen / oldLen) such that the
 * content point under `anchor` stays under `anchor`:
 *
 *   anchor = start  + k * len           // k = content fraction at anchor
 *   anchor = start' + k * len * ratio
 *   start' = anchor - (anchor - start) * ratio
 */
export function anchorStart(
  start: number,
  anchor: number,
  ratio: number
): number {
  return anchor - (anchor - start) * ratio;
}

/**
 * Keep the content on screen. When it is shorter than the viewport it is
 * centred; when longer, its edges may not come inside the viewport (no empty
 * margin can be dragged into view).
 */
export function clampStart(start: number, len: number, view: number): number {
  if (len <= view) return (view - len) / 2;
  return clamp(start, view - len, 0);
}

/**
 * Coalesce high-frequency input (wheel, mousemove) into at most one commit
 * per animation frame. Handlers mutate plain "pending" values synchronously
 * so back-to-back events inside a frame compose correctly, then call
 * `schedule()`; `commit` copies pending → reactive state once per frame, so
 * the CSS transform repaints at most once per frame however many events fire.
 */
export function createFrameCommit(commit: () => void) {
  let rafId: number | null = null;

  function schedule() {
    if (rafId !== null) return;
    rafId = requestAnimationFrame(() => {
      rafId = null;
      commit();
    });
  }

  function cancel() {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
  }

  /** Commit now and drop any queued frame (for rare, user-visible resets). */
  function flush() {
    cancel();
    commit();
  }

  return { schedule, cancel, flush };
}
