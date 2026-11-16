import { nextTick, onBeforeUnmount, onMounted, watch, type Ref } from "vue";
import type { VirtualGridHandle } from "@/components/VirtualGrid.vue";
import { activeGridCursor, type GridCursor } from "@/lib/gridCursor";
import { useViewTransition } from "@/composables/useViewTransition";

/**
 * Space kept clear above or below the cursor's row when following it: the
 * ring drawn outside the card and its hover lift, and on the first and last
 * rows the scroll container's own padding, so those scroll fully home.
 */
const FOLLOW_MARGIN = 16;

/**
 * The grid side of the keyboard cursor (see GridCursor). Keeps the store's
 * `currentIndex` in range and its card on screen: when the index changes
 * (keys, rating and advancing, a store reset), when the list changes under
 * the grid while the cursor is on screen (search, filters, a catalog
 * arriving, a live re-sort that the library store follows the current photo
 * through), and when the grid mounts outside a view transition. Scrolling
 * is left to VirtualGrid's `scrollToIndex`, which also makes the new
 * position the grid's scroll anchor.
 */
export function useGridCursor(options: {
  scroller: Ref<HTMLElement | null>;
  grid: Ref<VirtualGridHandle | null>;
  index: Ref<number>;
  count: () => number;
  list: () => unknown;
}) {
  const { scroller, grid, index, count } = options;
  const { activeTransitionId } = useViewTransition();

  /** Fully visible, partly visible, or not visible (or not mounted) at all. */
  function visibility(i: number): "full" | "part" | "none" {
    const el = scroller.value?.querySelector<HTMLElement>(`[data-index="${i}"]`);
    const card = el?.parentElement;
    if (!scroller.value || !card) return "none";
    const view = scroller.value.getBoundingClientRect();
    const r = card.getBoundingClientRect();
    if (r.top >= view.top && r.bottom <= view.bottom) return "full";
    if (r.bottom > view.top && r.top < view.bottom) return "part";
    return "none";
  }

  async function reveal(mode: "follow" | "return" = "follow") {
    // Let pending renders and VirtualGrid's re-anchoring after a list change
    // land first, so the target is computed against the current layout. Two
    // ticks: VirtualGrid writes the re-anchored scrollTop in a nextTick of
    // its own, queued in the same flush as this one.
    await nextTick();
    await nextTick();
    const n = count();
    if (n === 0 || !grid.value) return;
    // The library store keeps its index valid itself; the camera store
    // doesn't when its list is replaced.
    if (index.value > n - 1) index.value = n - 1;
    if (index.value < 0) index.value = 0;
    const i = index.value;
    if (mode === "return") {
      const seen = visibility(i);
      if (seen === "full") return;
      if (seen === "none") grid.value.scrollToIndex(i, "center");
      else grid.value.scrollToIndex(i, "nearest", FOLLOW_MARGIN);
    } else {
      grid.value.scrollToIndex(i, "nearest", FOLLOW_MARGIN);
    }
    await nextTick();
    noteOnScreen();
  }

  const handle: GridCursor = {
    get columns() {
      return grid.value?.columns ?? 1;
    },
    pageRows: () => grid.value?.pageRows() ?? 1,
    reveal,
  };

  // Whether the cursor's card was on screen as last rendered: the state the
  // user saw before a change. Updated after renders, scrolls and reveals.
  let onScreen = true;
  const noteOnScreen = () => {
    // While the list is empty (filters hiding everything) keep what was
    // seen before, for when the list comes back.
    if (count() > 0) onScreen = visibility(index.value) !== "none";
  };
  // Set from a list change until the end of that flush, so an index the
  // store moves in the same flush (the library re-anchoring its selection
  // after a re-sort) counts as part of the list change.
  let listChanging = false;

  // Moving the cursor (keys, rating and advancing, a click, a store reset on
  // the same list) always follows it. A list change (a live re-sort, ratings
  // landing, search, a new catalog) follows it only if it was on screen:
  // after a mouse scroll away from the cursor, VirtualGrid's scroll anchoring
  // keeps the user's place instead. An index the shorter list no longer has
  // is clamped (and shown) regardless.
  watch([options.list, index], ([list], [prevList]) => {
    if (list !== prevList && !listChanging) {
      listChanging = true;
      void nextTick(() => (listChanging = false));
    }
    if (!listChanging || onScreen || index.value > count() - 1) void reveal();
  });
  watch([options.list, index], noteOnScreen, { flush: "post" });
  onMounted(() => scroller.value?.addEventListener("scroll", noteOnScreen, { passive: true }));
  onBeforeUnmount(() => scroller.value?.removeEventListener("scroll", noteOnScreen));

  onMounted(() => {
    activeGridCursor.value = handle;
    // Back from the viewer, useViewTransition restores the old scroll
    // position first and then calls reveal("return"); revealing here as well
    // would nudge that position before it is restored.
    if (activeTransitionId.value === null) void reveal();
  });

  onBeforeUnmount(() => {
    if (activeGridCursor.value === handle) activeGridCursor.value = null;
  });
}
