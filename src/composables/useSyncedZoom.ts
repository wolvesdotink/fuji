import { ref, computed, onScopeDispose, type Ref, type ComputedRef } from "vue";
import {
  anchorStart,
  clamp,
  clampStart,
  createFrameCommit,
  wheelZoomFactor,
} from "@/lib/zoomPan";

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
 *
 * Shared math:
 *   The anchoring and clamping live in `@/lib/zoomPan` (also used by the
 *   single-image loupe). That module works with the displayed span's start
 *   `p` instead of the origin; in pane units the two are related by
 *   `p = originX * (1 - s)`, so clamping the origin to [0, 1] is the same
 *   as keeping the zoomed pane's edges outside the viewport.
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

/** Displayed span start (pane units) for a transform-origin at scale `s`. */
function originToStart(origin: number, s: number): number {
  return origin * (1 - s);
}

/** Inverse of originToStart. Undefined at s = 1, where any origin works. */
function startToOrigin(start: number, s: number): number {
  return s === 1 ? 0.5 : start / (1 - s);
}

export function useSyncedZoom(): SyncedZoom {
  const scale = ref(1);
  const originX = ref(0.5);
  const originY = ref(0.5);

  // Pending (uncommitted) zoom state. The wheel/drag handlers read AND
  // write these plain values synchronously so that back-to-back events
  // within a single frame compose correctly. Reading the reactive refs
  // instead would see stale values (they only update once per frame after
  // the rAF flush), making fast scroll-zoom drift or feel laggy. We commit
  // pending → refs once per animation frame; the CSS transform therefore
  // repaints at most once per frame no matter how many events fire.
  let pendingScale = scale.value;
  let pendingOriginX = originX.value;
  let pendingOriginY = originY.value;

  const frame = createFrameCommit(() => {
    scale.value = pendingScale;
    originX.value = pendingOriginX;
    originY.value = pendingOriginY;
  });

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

    // Read PENDING state — the latest value this frame, not the committed
    // ref which may lag a frame behind.
    const s = pendingScale;
    const newScale = clamp(
      s * wheelZoomFactor(e.deltaY),
      MIN_SCALE,
      MAX_SCALE
    );

    if (newScale <= MIN_SCALE) {
      // Fully zoomed out — reset to center to avoid a stale off-center
      // origin that would make the next zoom-in feel lopsided.
      pendingScale = MIN_SCALE;
      pendingOriginX = 0.5;
      pendingOriginY = 0.5;
      frame.schedule();
      return;
    }

    if (newScale === s) return;

    // Keep the image point under the cursor at the cursor, then keep the
    // zoomed pane covering the viewport. At the image edges the anchor
    // therefore can't hold perfectly, but there are never empty margins.
    const ratio = newScale / s;
    const startX = clampStart(
      anchorStart(originToStart(pendingOriginX, s), u, ratio),
      newScale,
      1
    );
    const startY = clampStart(
      anchorStart(originToStart(pendingOriginY, s), v, ratio),
      newScale,
      1
    );

    pendingScale = newScale;
    pendingOriginX = clamp(startToOrigin(startX, newScale), 0, 1);
    pendingOriginY = clamp(startToOrigin(startY, newScale), 0, 1);
    frame.schedule();
  }

  function onMouseDown(e: MouseEvent, pane: HTMLElement) {
    // Nothing to pan at scale 1 (whole image already visible).
    if (pendingScale <= MIN_SCALE) return;
    // Left button only — right-click and middle-click stay free for the
    // browser / future handlers.
    if (e.button !== 0) return;
    e.preventDefault();

    const rect = pane.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    const s = pendingScale;
    const startX = originToStart(pendingOriginX, s);
    const startY = originToStart(pendingOriginY, s);
    const startClientX = e.clientX;
    const startClientY = e.clientY;

    function onMove(ev: MouseEvent) {
      const du = (ev.clientX - startClientX) / rect.width;
      const dv = (ev.clientY - startClientY) / rect.height;
      // The span follows the cursor; in origin terms this is
      // originX' = originX - du/(s-1) (derivation in the header comment).
      // Writes pending; committed to the ref once per frame.
      pendingOriginX = clamp(startToOrigin(clampStart(startX + du, s, 1), s), 0, 1);
      pendingOriginY = clamp(startToOrigin(clampStart(startY + dv, s, 1), s), 0, 1);
      frame.schedule();
    }

    function onUp() {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    }

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  function reset() {
    pendingScale = 1;
    pendingOriginX = 0.5;
    pendingOriginY = 0.5;
    // Drop any queued frame so a stale wheel/pan commit can't clobber the
    // reset, then commit synchronously — reset is user-initiated and rare,
    // and the "Reset zoom" button's disabled state should flip immediately.
    frame.flush();
  }

  onScopeDispose(frame.cancel);

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
