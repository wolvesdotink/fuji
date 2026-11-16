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
import { computed, nextTick, onBeforeUnmount, onMounted, readonly, ref, toRaw, watch } from "vue";

const props = withDefaults(
  defineProps<{
    items: readonly T[];
    itemKey: (item: T) => string;
    /**
     * A stable, unique identity for the item across reloads (a full file
     * path, not the stem-only id). Lets the scroll anchor survive a list of
     * freshly created records for the same files; without it the anchor
     * follows object identity only.
     */
    anchorKey?: (item: T) => string;
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

/** Mount the rows around a scroll position of the scroll container. */
function setWindow(scrollTop: number) {
  const top = scrollTop - gridOffset();
  const bottom = top + (scrollEl?.clientHeight ?? 0);
  const first = Math.floor(top / rowPitch.value) - props.overscan;
  const last = Math.ceil(bottom / rowPitch.value) + props.overscan;
  startRow.value = Math.min(Math.max(first, 0), Math.max(totalRows.value - 1, 0));
  endRow.value = Math.max(Math.min(last, totalRows.value), startRow.value + 1);
}

/** Mount the rows for the current scroll position. */
function update() {
  if (!scrollEl || rowHeight.value <= 0) return;
  setWindow(scrollEl.scrollTop);
}

// ---------------------------------------------------------------------------
// Scroll anchoring
//
// Browser scroll anchoring is off for the grid (see the style block): with
// padding standing in for unmounted rows it would pick the wrong reference
// and correct a second time. So the grid owns anchoring completely.
//
// The anchor is the user's place in item terms: the first item of the first
// visible row and the unrounded pixel offset of that row from the viewport
// top. After a list change the item is found again by object identity, then
// by `anchorKey` (fresh records for the same files), then by its nearest
// surviving neighbour in the previous list. It is recorded only from user scrolling and from positions set
// deliberately (the viewer's scroll restore, scrollToIndex). Whenever the
// item → pixel mapping changes (column count or row height, the list, or the
// content above the grid) the anchor is re-applied, never re-derived from the
// rounded scrollTop that results, so repeated changes don't drift.
// ---------------------------------------------------------------------------

interface Anchor {
  item: T;
  /** Index of `item` when last resolved; the fallback if it disappears. */
  index: number;
  /** Pixels from the top of the item's row to the top of the viewport. */
  offset: number;
  atBottom: boolean;
}

/** Null at the top of the grid: there the pixel position is already right. */
let anchor: Anchor | null = null;
/** The scrollTop the grid last wrote itself; its scroll events aren't the user's. */
let programmaticTop: number | null = null;

// Identity → index and anchorKey → index for the current list, rebuilt once
// per list change so the anchor lookup after a reorder is O(1).
let listItems: readonly T[] = [];
let indexOf = new Map<unknown, number>();
let keyIndexOf = new Map<string, number>();

/** Index of `item` (or of a fresh record for the same file) in the current list. */
function resolveIndex(item: T): number | undefined {
  const hit = indexOf.get(toRaw(item));
  if (hit !== undefined || !props.anchorKey) return hit;
  return keyIndexOf.get(props.anchorKey(item));
}

function recordAnchor() {
  if (!scrollEl || rowHeight.value <= 0) return;
  const top = scrollEl.scrollTop - gridOffset();
  if (scrollEl.scrollTop <= 0 || top <= 0 || props.items.length === 0) {
    anchor = null;
    return;
  }
  const row = Math.min(Math.floor(top / rowPitch.value), totalRows.value - 1);
  const index = row * columns.value;
  anchor = {
    item: props.items[index],
    index,
    offset: top - row * rowPitch.value,
    atBottom: scrollEl.scrollTop >= scrollEl.scrollHeight - scrollEl.clientHeight - 1,
  };
}

/** Where the anchor puts the viewport top, in scroll-container pixels. */
function anchorTarget(a: Anchor): number {
  const offset = gridOffset();
  if (a.atBottom && scrollEl) {
    const padBottom = parseFloat(getComputedStyle(scrollEl).paddingBottom) || 0;
    const contentEnd = offset + totalRows.value * rowPitch.value - rowGap.value + padBottom;
    return Math.max(contentEnd - scrollEl.clientHeight, 0);
  }
  const hit = resolveIndex(a.item);
  const index = hit ?? Math.min(Math.max(a.index, 0), Math.max(props.items.length - 1, 0));
  a.index = index;
  return offset + Math.floor(index / columns.value) * rowPitch.value + a.offset;
}

let reanchorQueued = false;

/**
 * Re-apply the anchor after the item → pixel mapping changed. The rows for
 * the target are mounted right away; scrollTop is written after Vue has
 * patched the padding (until then the content is sized for the old mapping
 * and the browser would clamp it). Both happen before the next paint.
 */
function reanchor() {
  if (!scrollEl || rowHeight.value <= 0) return;
  if (!anchor) {
    update();
    return;
  }
  setWindow(anchorTarget(anchor));
  if (reanchorQueued) return;
  reanchorQueued = true;
  nextTick(() => {
    reanchorQueued = false;
    if (!scrollEl || !anchor) return;
    const max = scrollEl.scrollHeight - scrollEl.clientHeight;
    // Rounded only here: WebKit keeps integral scroll positions and would
    // truncate a fraction. The anchor itself stays unrounded.
    const target = Math.round(anchor.atBottom ? max : Math.min(anchorTarget(anchor), max));
    if (scrollEl.scrollTop !== target) scrollEl.scrollTop = target;
    programmaticTop = scrollEl.scrollTop;
    update();
  });
}

/** Write scrollTop and make that position the new anchor. */
function scrollToPosition(scrollTop: number) {
  if (!scrollEl) return;
  scrollEl.scrollTop = Math.round(Math.max(scrollTop, 0));
  programmaticTop = scrollEl.scrollTop;
  recordAnchor();
  update();
}

/**
 * Re-read the geometry; re-apply the anchor if the column count or row
 * height changed. Unchanged geometry, including the first measurement on
 * mount, leaves scrollTop alone.
 */
function remeasure() {
  const before = rowHeight.value > 0 ? { columns: columns.value, pitch: rowPitch.value } : null;
  measure();
  const changed =
    before !== null &&
    (before.columns !== columns.value || Math.abs(before.pitch - rowPitch.value) > 0.001);
  if (changed) reanchor();
  else update();
}

/**
 * Scroll so that item `index` is visible. `block: "nearest"` scrolls the
 * least amount needed (nothing if it is already fully visible); `"center"`
 * centres its row. `margin` (pixels) is kept clear above or below the row
 * with "nearest", for decorations drawn outside the card such as the keyboard
 * cursor's ring. The rows are mounted synchronously, but the cards are in
 * the DOM only after Vue's next render: wait a tick (`await nextTick()`)
 * before querying them.
 */
function scrollToIndex(index: number, block: "nearest" | "center" = "nearest", margin = 0) {
  if (!scrollEl || rowHeight.value <= 0 || index < 0 || index >= props.items.length) return;
  const row = Math.floor(index / columns.value);
  const rowTop = gridOffset() + row * rowPitch.value - margin;
  const rowBottom = rowTop + rowHeight.value + 2 * margin;
  const viewTop = scrollEl.scrollTop;
  const viewHeight = scrollEl.clientHeight;
  let target = viewTop;
  if (block === "center") {
    target = rowTop + margin - (viewHeight - rowHeight.value) / 2;
  } else if (rowTop < viewTop) {
    target = rowTop;
  } else if (rowBottom > viewTop + viewHeight) {
    target = rowBottom - viewHeight;
  }
  scrollToPosition(target);
}

/** Rows that fit in the viewport (at least 1): the step for Page Up / Page Down. */
function pageRows(): number {
  if (!scrollEl || rowPitch.value <= 0) return 1;
  return Math.max(Math.floor((scrollEl.clientHeight + rowGap.value) / rowPitch.value), 1);
}

defineExpose({
  scrollToIndex,
  pageRows,
  /** Current column count; reactive, readonly. */
  columns: readonly(columns),
});

// Scroll events are handled synchronously (no rAF throttle): WebKit already
// fires them once per frame, and updating in the same frame keeps freshly
// exposed rows from being painted blank. useViewTransition restores a scroll
// position by dispatching a synthetic scroll event, so the matching rows are
// mounted before the view-transition snapshot; an untrusted event is taken as
// such a deliberate position and always sets the anchor.
function onScroll(e: Event) {
  if (!scrollEl || rowHeight.value <= 0) return;
  const echo =
    e.isTrusted &&
    programmaticTop !== null &&
    Math.abs(scrollEl.scrollTop - programmaticTop) < 0.5;
  if (!echo) {
    programmaticTop = null;
    recordAnchor();
  }
  update();
}

/**
 * The list changed. Find the anchored item again: the same object (a sort,
 * ratings arriving under the Stars sort, inserts and removals elsewhere), else
 * a fresh record for the same file by `anchorKey` (the same folder or catalog
 * reloaded). If it is gone, take its nearest surviving neighbour in the
 * previous list, following side first so a removal doesn't shift the view;
 * the scan covers the whole previous list, once per change. Only a list with
 * no survivor at all (a different folder or card) starts at the top.
 */
function onListChange(prevItems: readonly T[], prevIndexOf: Map<unknown, number>) {
  if (!anchor || prevItems.length === 0) {
    update();
    return;
  }
  let index = resolveIndex(anchor.item);
  if (index === undefined) {
    const from = prevIndexOf.get(toRaw(anchor.item)) ?? anchor.index;
    for (let d = 1; index === undefined && (from + d < prevItems.length || from - d >= 0); d++) {
      for (const i of [from + d, from - d]) {
        if (i < 0 || i >= prevItems.length) continue;
        const hit = resolveIndex(prevItems[i]);
        if (hit !== undefined) {
          index = hit;
          break;
        }
      }
    }
    if (index === undefined) {
      anchor = null;
      scrollToPosition(0);
      return;
    }
  }
  anchor.item = props.items[index];
  anchor.index = index;
  reanchor();
}

let resizeObserver: ResizeObserver | null = null;
let mutationObserver: MutationObserver | null = null;

onMounted(() => {
  scrollEl = gridEl.value?.parentElement ?? null;
  scrollEl?.addEventListener("scroll", onScroll, { passive: true });
  remeasure();
  // That first pass measured a single card in an otherwise empty grid, so the
  // scroll container may not have had its scrollbar yet, and the column width
  // (hence card height) can be off by the scrollbar's width. Give the grid its
  // full height now and measure again, so the geometry is final before
  // useViewTransition restores a scroll position against it.
  if (gridEl.value && rowHeight.value > 0) {
    Object.assign(gridEl.value.style, padding.value);
    remeasure();
  }

  // Card height only depends on the column width and the footer's text
  // metrics. The scroll container's content box covers the first (window
  // resizes, and a classic scrollbar appearing or going away); font loads
  // cover the second. Cards themselves are not observed: a new card's first
  // observation would land mid-update and trip the ResizeObserver loop limit.
  resizeObserver = new ResizeObserver(remeasure);
  if (scrollEl) resizeObserver.observe(scrollEl);
  document.fonts.addEventListener("loadingdone", remeasure);

  // Content above the grid in the scroll container (the thumbnail loading
  // bar, the library loading indicator) comes and goes as direct children;
  // that moves every row, so keep the anchored item in place.
  if (scrollEl) {
    mutationObserver = new MutationObserver(reanchor);
    mutationObserver.observe(scrollEl, { childList: true });
  }
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
  [() => props.items, () => props.items.length],
  () => {
    const prevItems = listItems;
    const prevIndexOf = indexOf;
    listItems = props.items.slice();
    indexOf = new Map(listItems.map((item, i) => [toRaw(item), i]));
    const anchorKey = props.anchorKey;
    keyIndexOf = anchorKey ? new Map(listItems.map((item, i) => [anchorKey(item), i])) : new Map();
    if (rowHeight.value === 0) {
      // Nothing measured yet: render one item so the watcher above can.
      endRow.value = 1;
      return;
    }
    onListChange(prevItems, prevIndexOf);
  },
  { immediate: true }
);

onBeforeUnmount(() => {
  scrollEl?.removeEventListener("scroll", onScroll);
  document.fonts.removeEventListener("loadingdone", remeasure);
  resizeObserver?.disconnect();
  resizeObserver = null;
  mutationObserver?.disconnect();
  mutationObserver = null;
});
</script>

<script lang="ts">
/**
 * What VirtualGrid (and ImageGrid / LibraryGrid, which forward it) exposes
 * through a template ref. After scrollToIndex() the target cards are mounted
 * on Vue's next render: `await nextTick()` before querying them.
 */
export interface VirtualGridHandle {
  scrollToIndex(index: number, block?: "nearest" | "center", margin?: number): void;
  /** Rows that fit in the viewport (at least 1). */
  pageRows(): number;
  /** Current column count. Reactive when read through the component ref. */
  readonly columns: number;
}
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
  /* The component anchors the scroll position itself (see "Scroll
     anchoring" above); the browser's anchoring would correct a second time
     whenever the padding changes. */
  overflow-anchor: none;
}
</style>
