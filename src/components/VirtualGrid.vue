<script setup lang="ts" generic="T">
/**
 * Windowed photo grid shared by the camera and library grids.
 *
 * Only the rows near the viewport (plus `overscan` rows either side) are
 * mounted. The rows above and below the window are stood in for by padding on
 * the grid element, so the scroll height, the scrollbar and every mounted
 * card's position match a fully rendered grid exactly.
 *
 * Layout is still plain CSS grid (`repeat(auto-fill, minmax(250px, 1fr))`):
 * the column count is read back from the computed track list rather than
 * re-derived, and the row pitch is measured from a mounted card, so the
 * windowing math can never disagree with what the browser lays out. All cards
 * in a grid have the same height for a given column width (3:2 thumbnail plus
 * a fixed footer), which is what makes fixed-pitch windowing possible.
 *
 * The scroll container is the grid's parent element (`.image-grid-container`
 * / `.library-grid-container`), which also holds the loading bar and the
 * empty/loading states above the grid.
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

const props = withDefaults(
  defineProps<{
    items: readonly T[];
    itemKey: (item: T) => string;
    /** Rows mounted above and below the visible ones. */
    overscan?: number;
  }>(),
  { overscan: 3 }
);

defineSlots<{
  default(props: { item: T; index: number }): unknown;
}>();

const gridEl = ref<HTMLElement | null>(null);
let scrollEl: HTMLElement | null = null;

// Layout, measured from the DOM. Until the first measurement the grid renders
// a single item, which is enough to read the real card height in onMounted
// before anything is painted.
const columns = ref(1);
const rowHeight = ref(0);
const rowGap = ref(0);
const startRow = ref(0);
const endRow = ref(1);

const rowPitch = computed(() => rowHeight.value + rowGap.value);
const totalRows = computed(() => Math.ceil(props.items.length / columns.value));

const startIndex = computed(() => startRow.value * columns.value);
const visibleItems = computed(() =>
  props.items.slice(startIndex.value, Math.min(endRow.value * columns.value, props.items.length))
);

const padding = computed(() => {
  if (rowHeight.value === 0) return { paddingTop: "0px", paddingBottom: "0px" };
  const end = Math.min(endRow.value, totalRows.value);
  const after = Math.max(totalRows.value - end, 0);
  return {
    paddingTop: `${startRow.value * rowPitch.value}px`,
    paddingBottom: `${after * rowPitch.value}px`,
  };
});

/** Distance from the top of the scroll content to the top of the grid. */
function gridOffset(): number {
  if (!gridEl.value || !scrollEl) return 0;
  return (
    gridEl.value.getBoundingClientRect().top -
    scrollEl.getBoundingClientRect().top +
    scrollEl.scrollTop
  );
}

/** Read column count, row gap and card height back from the live layout. */
function measure() {
  const grid = gridEl.value;
  if (!grid) return;
  const style = getComputedStyle(grid);
  const tracks = style.gridTemplateColumns.split(" ").filter(Boolean).length;
  columns.value = Math.max(tracks, 1);
  rowGap.value = parseFloat(style.rowGap) || 0;
  const card = grid.firstElementChild as HTMLElement | null;
  if (card) {
    // getBoundingClientRect keeps the fractional height; offsetHeight would
    // round, and the error would add up over thousands of rows.
    const h = card.getBoundingClientRect().height;
    if (h > 0) rowHeight.value = h;
  }
}

/** Recompute which rows are mounted from the current scroll position. */
function update() {
  if (!scrollEl || rowHeight.value <= 0) return;
  const top = scrollEl.scrollTop - gridOffset();
  const bottom = top + scrollEl.clientHeight;
  const first = Math.floor(top / rowPitch.value) - props.overscan;
  const last = Math.ceil(bottom / rowPitch.value) + props.overscan;
  startRow.value = Math.min(Math.max(first, 0), Math.max(totalRows.value - 1, 0));
  endRow.value = Math.max(Math.min(last, totalRows.value), startRow.value + 1);
}

function remeasure() {
  measure();
  update();
}

/**
 * Scroll so that item `index` is visible. `block: "nearest"` scrolls the
 * least amount needed (nothing if it is already fully visible); `"center"`
 * centres its row. The new rows are mounted synchronously.
 */
function scrollToIndex(index: number, block: "nearest" | "center" = "nearest") {
  if (!scrollEl || rowHeight.value <= 0 || index < 0 || index >= props.items.length) return;
  const row = Math.floor(index / columns.value);
  const rowTop = gridOffset() + row * rowPitch.value;
  const rowBottom = rowTop + rowHeight.value;
  const viewTop = scrollEl.scrollTop;
  const viewHeight = scrollEl.clientHeight;
  let target = viewTop;
  if (block === "center") {
    target = rowTop - (viewHeight - rowHeight.value) / 2;
  } else if (rowTop < viewTop) {
    target = rowTop;
  } else if (rowBottom > viewTop + viewHeight) {
    target = rowBottom - viewHeight;
  }
  scrollEl.scrollTop = Math.max(target, 0);
  update();
}

defineExpose({ scrollToIndex });

// Scroll events are handled synchronously (no rAF throttle): WebKit already
// fires them once per frame, and updating in the same frame keeps freshly
// exposed rows from being painted blank. It also lets useViewTransition
// restore a scroll position and have the matching rows mounted before the
// view-transition snapshot is taken.
function onScroll() {
  update();
}

let resizeObserver: ResizeObserver | null = null;

onMounted(() => {
  scrollEl = gridEl.value?.parentElement ?? null;
  scrollEl?.addEventListener("scroll", onScroll, { passive: true });
  remeasure();

  // Card height only depends on the column width and the footer's text
  // metrics. The scroll container's content box covers the first (window
  // resizes, and a classic scrollbar appearing or going away); font loads
  // cover the second. Cards themselves are not observed: a new card's first
  // observation would land mid-update and trip the ResizeObserver loop limit.
  resizeObserver = new ResizeObserver(remeasure);
  if (scrollEl) resizeObserver.observe(scrollEl);
  document.fonts.addEventListener("loadingdone", remeasure);
});

watch(
  visibleItems,
  () => {
    // Runs after the DOM patch: take the first real measurement if items
    // only just arrived.
    if (rowHeight.value === 0 && gridEl.value?.firstElementChild) remeasure();
  },
  { flush: "post" }
);

watch(
  () => props.items.length,
  () => {
    if (rowHeight.value === 0) {
      // Nothing measured yet: render one item so the watcher above can.
      endRow.value = 1;
      return;
    }
    // The list may have shrunk below (or grown past) the current window.
    update();
  },
  { flush: "post" }
);

onBeforeUnmount(() => {
  scrollEl?.removeEventListener("scroll", onScroll);
  document.fonts.removeEventListener("loadingdone", remeasure);
  resizeObserver?.disconnect();
  resizeObserver = null;
});
</script>

<template>
  <div ref="gridEl" class="virtual-grid" :style="padding">
    <template v-for="(item, i) in visibleItems" :key="itemKey(item)">
      <slot :item="item" :index="startIndex + i" />
    </template>
  </div>
</template>

<style scoped>
.virtual-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 14px;
}
</style>
