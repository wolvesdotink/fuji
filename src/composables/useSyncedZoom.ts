import { ref, computed, type Ref, type ComputedRef } from "vue";

/**
 * Shared zoom + pan state for N side-by-side panes.
 *
 * This is a **factory** — call it once at the root of the component that
 * owns the compare view (e.g. `ImageCompare.vue`). Every pane in that
 * component then shares the same `scale` / `origin` refs, so scroll-wheel
 * zoom and drag-pan on any pane applies to all of them pixel-for-pixel.
 *
 * Why a factory (not a module-level singleton): entering and leaving
 * compare mode remounts `ImageCompare`, and HMR can remount it more. A
 * singleton would retain stale zoom state across those transitions; a
 * factory resets cleanly with the component.
 *
 * Coordinate model:
 *   - `scale` ∈ [1, 16]. We intentionally do not allow scale < 1 — the
 *     compare use-case is pixel-peeping, not "see the whole frame zoomed
 *     out", and clamping at 1 avoids a division-by-zero edge case in the
 *     pan math below.
 *   - `originX`, `originY` ∈ [0, 1] — normalized so panes of different
 *     pixel sizes stay in sync. These drive `transform-origin`.
 *   - The actual CSS transform is just `scale(s)`; positioning comes from
 *     `transform-origin: (originX*100%) (originY*100%)`. This means the
 *     image point at (originX, originY) of the pane is pinned to that
 *     relative screen location, and everything else scales around it.
 *
 * Zoom anchoring:
 *   Wheel events zoom "toward the cursor" — the image point under the
 *   cursor stays under the cursor across the zoom. See `onWheel`.
 *
 * Pan math:
 *   At scale `s`, moving the cursor by `du` normalized units should move
 *   the image point currently under the cursor with it. Working through
 *   the transform algebra:
 *
 *     u       = ix*s + originX*(1-s)          // cursor at ix in image
 *     u + du  = ix*s + originX'*(1-s)         // after drag
 *     originX' = originX - du/(s-1)           // solve for new origin
 *
 *   That's why pan is disabled at s=1: with nothing zoomed in, there's
 *   nothing to pan.
 */

export interface SyncedZoom {
  scale: Ref<number>;
  originX: Ref<number>;
  originY: Ref<number>;
  transform: ComputedRef<string>;
  transformOrigin: ComputedRef<string>;
  onWheel(e: WheelEvent, pane: HTMLElement): void;
  onMouseDown(e: MouseEvent, pane: HTMLElement): void;
  reset(): void;
}

const MIN_SCALE = 1;
const MAX_SCALE = 16;

function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, v));
}

export function useSyncedZoom(): SyncedZoom {
  const scale = ref(1);
  const originX = ref(0.5);
  const originY = ref(0.5);

  const transform = computed(() => `scale(${scale.value})`);
  const transformOrigin = computed(
    () => `${originX.value * 100}% ${originY.value * 100}%`
  );

  function onWheel(e: WheelEvent, pane: HTMLElement) {
    e.preventDefault();
    const rect = pane.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    // Cursor position within pane, normalized 0..1.
    const u = (e.clientX - rect.left) / rect.width;
    const v = (e.clientY - rect.top) / rect.height;

    const s = scale.value;
    // Image point currently under the cursor. At s=1 the transform is
    // identity, so the image point equals the cursor position directly.
    const ix = s === 1 ? u : (u - originX.value * (1 - s)) / s;
    const iy = s === 1 ? v : (v - originY.value * (1 - s)) / s;

    // Exponential zoom feels more natural than linear. ~120 per wheel
    // "click" on typical mice → factor ≈ e^(-0.24) ≈ 0.79 per tick out.
    const zoomFactor = Math.exp(-e.deltaY * 0.002);
    const newScale = clamp(s * zoomFactor, MIN_SCALE, MAX_SCALE);

    if (newScale <= MIN_SCALE) {
      // Fully zoomed out — reset to center to avoid a stale off-center
      // origin that would make the next zoom-in feel lopsided.
      scale.value = MIN_SCALE;
      originX.value = 0.5;
      originY.value = 0.5;
      return;
    }

    if (newScale === s) return;

    // Keep (ix, iy) at cursor (u, v):
    //   u = ix*newScale + newOrigin*(1-newScale)
    //   newOrigin = (u - ix*newScale) / (1 - newScale)
    const rawOriginX = (u - ix * newScale) / (1 - newScale);
    const rawOriginY = (v - iy * newScale) / (1 - newScale);

    scale.value = newScale;
    // Clamping origin to [0,1] means at image edges the cursor-anchor
    // won't hold perfectly, but it guarantees the transform-origin stays
    // within the image (no surprise empty margins).
    originX.value = clamp(rawOriginX, 0, 1);
    originY.value = clamp(rawOriginY, 0, 1);
  }

  function onMouseDown(e: MouseEvent, pane: HTMLElement) {
    // Nothing to pan at scale 1 (whole image already visible).
    if (scale.value <= MIN_SCALE) return;
    // Left button only — right-click and middle-click stay free for the
    // browser / future handlers.
    if (e.button !== 0) return;
    e.preventDefault();

    const rect = pane.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    const startOriginX = originX.value;
    const startOriginY = originY.value;
    const startClientX = e.clientX;
    const startClientY = e.clientY;
    const s = scale.value;

    function onMove(ev: MouseEvent) {
      const du = (ev.clientX - startClientX) / rect.width;
      const dv = (ev.clientY - startClientY) / rect.height;
      // Pan math: originX' = originX - du/(s-1). Derivation in the file
      // header comment.
      originX.value = clamp(startOriginX - du / (s - 1), 0, 1);
      originY.value = clamp(startOriginY - dv / (s - 1), 0, 1);
    }

    function onUp() {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    }

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  function reset() {
    scale.value = 1;
    originX.value = 0.5;
    originY.value = 0.5;
  }

  return {
    scale,
    originX,
    originY,
    transform,
    transformOrigin,
    onWheel,
    onMouseDown,
    reset,
  };
}
