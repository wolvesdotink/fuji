<script setup lang="ts">
import { ref, computed, nextTick, watch, onBeforeUnmount } from "vue";
import { useLibraryStore } from "@/stores/library";
import {
  RATING_FILTERS,
  MEDIA_FILTERS,
  ratingFilterLabel,
  describeFilters,
  type RatingFilter,
  type MediaFilter,
} from "@/lib/libraryFilter";

const store = useLibraryStore();

const STAR_PATH =
  "M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z";
const POPOVER_ID = "library-filter-popover";

const open = ref(false);
const root = ref<HTMLElement | null>(null);
const trigger = ref<HTMLButtonElement | null>(null);
const popover = ref<HTMLElement | null>(null);

const activeCount = computed(
  () => (store.ratingFilter !== "all" ? 1 : 0) + (store.mediaFilter !== "all" ? 1 : 0)
);

const summary = computed(() => describeFilters(store.ratingFilter, store.mediaFilter));

const triggerLabel = computed(() =>
  store.isFiltered ? `Filter: ${summary.value}` : "Filter"
);

// Counts shown next to each option. Each group is counted against the other
// group's current choice, so the numbers are what you get by clicking.
function sum(h: number[], from: number) {
  let n = 0;
  for (let i = from; i < h.length; i++) n += h[i];
  return n;
}

const ratingHistogram = computed(() => {
  const { photo, video } = store.filterCounts;
  if (store.mediaFilter === "Image") return photo;
  if (store.mediaFilter === "Video") return video;
  return photo.map((n, i) => n + video[i]);
});

function ratingCount(f: RatingFilter) {
  const h = ratingHistogram.value;
  if (f === "all") return sum(h, 0);
  if (f === "unrated") return h[0];
  return sum(h, f);
}

function mediaCount(m: MediaFilter) {
  const { photo, video } = store.filterCounts;
  const rf = store.ratingFilter;
  const pick = (h: number[]) =>
    rf === "all" ? sum(h, 0) : rf === "unrated" ? h[0] : sum(h, rf);
  if (m === "Image") return pick(photo);
  if (m === "Video") return pick(video);
  return pick(photo) + pick(video);
}

// --- Open / close --------------------------------------------------------

function focusChecked() {
  popover.value
    ?.querySelector<HTMLElement>('.rating-option[aria-checked="true"]')
    ?.focus();
}

function openPopover() {
  open.value = true;
  nextTick(focusChecked);
}

function closePopover(returnFocus: boolean) {
  if (!open.value) return;
  open.value = false;
  if (returnFocus) trigger.value?.focus();
}

function toggle() {
  if (open.value) closePopover(true);
  else openPopover();
}

function clear() {
  store.clearFilters();
}

// Outside interaction closes the popover. Listeners exist only while open.
function onPointerDownOutside(e: PointerEvent) {
  if (root.value && !root.value.contains(e.target as Node)) closePopover(false);
}

function onFocusInOutside(e: FocusEvent) {
  if (root.value && !root.value.contains(e.target as Node)) closePopover(false);
}

// Capture phase on window runs before useKeyboardNav's bubble-phase window
// listener, so stopping Esc here keeps it from also clearing the search or
// leaving the viewer. Works even if focus has wandered off the popover.
function onEscapeCapture(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  e.preventDefault();
  e.stopPropagation();
  closePopover(true);
}

watch(open, (isOpen) => {
  if (isOpen) {
    document.addEventListener("pointerdown", onPointerDownOutside, true);
    document.addEventListener("focusin", onFocusInOutside);
    window.addEventListener("keydown", onEscapeCapture, true);
  } else {
    removeListeners();
  }
});

function removeListeners() {
  document.removeEventListener("pointerdown", onPointerDownOutside, true);
  document.removeEventListener("focusin", onFocusInOutside);
  window.removeEventListener("keydown", onEscapeCapture, true);
}

onBeforeUnmount(removeListeners);

// --- Keyboard --------------------------------------------------------------

// Keys the trigger button uses itself. Space would otherwise also reach the
// global handler (play/pause on a video in the viewer).
const TRIGGER_KEYS = new Set([" ", "Enter", "ArrowDown", "ArrowUp"]);

function onControlKeydown(e: KeyboardEvent) {
  if (!open.value) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      openPopover();
    }
    // Closed, the trigger behaves like the other header buttons: digits and
    // left/right still rate and navigate. That matters because focus lands
    // back here after Esc, and culling must keep working.
    if (TRIGGER_KEYS.has(e.key)) e.stopPropagation();
    return;
  }
  // Open, everything typed in the control stays in it, so digits and arrows
  // drive the popover instead of rating or navigating photos.
  e.stopPropagation();
}

/** Roving focus for a radiogroup: arrows move and select, like native radios. */
function onRadioKeydown<T>(
  e: KeyboardEvent,
  values: readonly T[],
  current: T,
  select: (v: T) => void
) {
  const i = values.indexOf(current);
  let next = -1;
  switch (e.key) {
    case "ArrowDown":
    case "ArrowRight":
      next = (i + 1) % values.length;
      break;
    case "ArrowUp":
    case "ArrowLeft":
      next = (i - 1 + values.length) % values.length;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = values.length - 1;
      break;
    default:
      return;
  }
  e.preventDefault();
  select(values[next]);
  const group = (e.currentTarget as HTMLElement).closest('[role="radiogroup"]');
  nextTick(() =>
    group?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus()
  );
}

const mediaValues = MEDIA_FILTERS.map((m) => m.value);

function onRatingKeydown(e: KeyboardEvent) {
  onRadioKeydown(e, RATING_FILTERS, store.ratingFilter, store.setRatingFilter);
}

function onMediaKeydown(e: KeyboardEvent) {
  onRadioKeydown(e, mediaValues, store.mediaFilter, store.setMediaFilter);
}
</script>

<template>
  <div ref="root" class="filter-control" @keydown="onControlKeydown">
    <div :class="['filter-pill', { active: store.isFiltered, open }]">
      <button
        ref="trigger"
        type="button"
        class="filter-trigger"
        aria-haspopup="dialog"
        :aria-expanded="open"
        :aria-controls="POPOVER_ID"
        :aria-label="triggerLabel"
        :title="triggerLabel"
        @click="toggle"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M3 5h18M6.5 12h11M10 19h4" />
        </svg>
        <span v-if="!store.isFiltered" class="filter-text">Filter</span>
        <span v-else class="filter-text filter-summary" aria-hidden="true">
          <template v-if="typeof store.ratingFilter === 'number'">
            <svg class="summary-star" viewBox="0 0 24 24" fill="currentColor"><path :d="STAR_PATH" /></svg>
            {{ store.ratingFilter }}{{ store.ratingFilter < 5 ? "+" : "" }}
          </template>
          <template v-else-if="store.ratingFilter === 'unrated'">Unrated</template>
          <span v-if="activeCount === 2" class="summary-sep">&middot;</span>
          <template v-if="store.mediaFilter !== 'all'">
            {{ store.mediaFilter === "Video" ? "Videos" : "Photos" }}
          </template>
        </span>
        <span v-if="store.isFiltered" class="filter-badge" aria-hidden="true">{{ activeCount }}</span>
      </button>
      <button
        v-if="store.isFiltered"
        type="button"
        class="filter-clear"
        aria-label="Clear filters"
        title="Clear filters"
        @click="clear"
      >
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" aria-hidden="true">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </button>
    </div>

    <Transition name="filter-pop">
      <div
        v-if="open"
        :id="POPOVER_ID"
        ref="popover"
        class="filter-popover"
        role="dialog"
        aria-label="Filter library"
        tabindex="-1"
      >
        <div class="section-label" id="filter-rating-label">Rating</div>
        <div class="rating-options" role="radiogroup" aria-labelledby="filter-rating-label">
          <button
            v-for="f in RATING_FILTERS"
            :key="String(f)"
            type="button"
            role="radio"
            :aria-checked="store.ratingFilter === f"
            :tabindex="store.ratingFilter === f ? 0 : -1"
            :aria-label="`${ratingFilterLabel(f)}, ${ratingCount(f)}`"
            :class="['rating-option', { checked: store.ratingFilter === f, empty: ratingCount(f) === 0 }]"
            @click="store.setRatingFilter(f)"
            @keydown="onRatingKeydown"
          >
            <span class="option-mark" aria-hidden="true"></span>
            <span class="option-body" aria-hidden="true">
              <template v-if="f === 'all'">Any rating</template>
              <template v-else-if="f === 'unrated'">
                <svg class="option-star hollow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path :d="STAR_PATH" /></svg>
                Unrated
              </template>
              <template v-else>
                <span class="option-stars">
                  <svg
                    v-for="s in 5"
                    :key="s"
                    :class="['option-star', { filled: s <= f }]"
                    viewBox="0 0 24 24"
                    fill="currentColor"
                  ><path :d="STAR_PATH" /></svg>
                </span>
                <span v-if="f < 5" class="option-suffix">&amp; up</span>
              </template>
            </span>
            <span class="option-count" aria-hidden="true">{{ ratingCount(f) }}</span>
          </button>
        </div>

        <div class="section-label" id="filter-media-label">Media</div>
        <div class="media-toggle" role="radiogroup" aria-labelledby="filter-media-label">
          <button
            v-for="m in MEDIA_FILTERS"
            :key="m.value"
            type="button"
            role="radio"
            :aria-checked="store.mediaFilter === m.value"
            :tabindex="store.mediaFilter === m.value ? 0 : -1"
            :aria-label="`${m.label}, ${mediaCount(m.value)}`"
            :class="['toggle-btn', { active: store.mediaFilter === m.value }]"
            @click="store.setMediaFilter(m.value)"
            @keydown="onMediaKeydown"
          >
            {{ m.label }}
            <span class="media-count" aria-hidden="true">{{ mediaCount(m.value) }}</span>
          </button>
        </div>

        <div class="filter-footer">
          <button
            type="button"
            class="footer-clear"
            :disabled="!store.isFiltered"
            @click="clear"
          >Clear filters</button>
          <kbd class="footer-hint" aria-hidden="true">esc</kbd>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.filter-control {
  position: relative;
  flex-shrink: 0;
}

/* Trigger: same container as the sort and view toggles. */
.filter-pill {
  display: flex;
  align-items: center;
  background: var(--color-bg);
  border: 1px solid var(--color-border-subtle);
  border-radius: var(--radius-sm);
  padding: 2px;
  transition: border-color var(--transition-fast);
}

.filter-trigger,
.filter-clear {
  background: none;
  border: none;
  color: var(--color-text-muted);
  border-radius: 4px;
  cursor: pointer;
  display: flex;
  align-items: center;
  font-family: var(--font-body);
  font-size: 11px;
  font-weight: 500;
  white-space: nowrap;
  transition: all var(--transition-fast);
}

.filter-trigger {
  gap: 6px;
  padding: 5px 10px;
}

.filter-trigger:hover {
  color: var(--color-text-secondary);
}

.filter-pill.open .filter-trigger {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

/* Active: gold, like a lit indicator on the camera body. */
.filter-pill.active {
  border-color: rgba(196, 162, 78, 0.35);
}

.filter-pill.active .filter-trigger {
  background: var(--color-accent-dim);
  color: var(--color-accent);
}

.filter-pill.active .filter-trigger:hover,
.filter-pill.active.open .filter-trigger {
  color: var(--color-accent-hover);
}

.filter-summary {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  font-variant-numeric: tabular-nums;
}

.summary-star {
  width: 11px;
  height: 11px;
}

.summary-sep {
  opacity: 0.6;
  margin: 0 2px;
}

.filter-badge {
  display: none;
  min-width: 15px;
  height: 15px;
  padding: 0 4px;
  border-radius: 8px;
  background: var(--color-accent);
  color: var(--color-bg);
  font-size: 10px;
  font-weight: 700;
  line-height: 15px;
  text-align: center;
  font-variant-numeric: tabular-nums;
}

.filter-clear {
  padding: 6px 7px;
  margin-left: 1px;
}

.filter-clear:hover {
  color: var(--color-text);
  background: var(--color-surface-hover);
}

/* Narrow windows: the header is tight, so the trigger drops to an icon with
   a count badge. The full summary stays in the tooltip and accessible name. */
@media (max-width: 1100px) {
  .filter-text {
    display: none;
  }
  .filter-trigger {
    padding: 5px 7px;
  }
  .filter-badge {
    display: inline-block;
  }
}

/* Popover */
.filter-popover {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  z-index: 60;
  width: 228px;
  padding: 6px;
  background: var(--color-surface-elevated);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  box-shadow: 0 18px 40px rgba(0, 0, 0, 0.55), 0 2px 6px rgba(0, 0, 0, 0.35);
  outline: none;
}

.section-label {
  padding: 8px 8px 5px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--color-text-muted);
}

.rating-options {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.rating-option {
  display: grid;
  grid-template-columns: 6px 1fr auto;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 6px 8px;
  background: none;
  border: none;
  border-radius: 5px;
  color: var(--color-text-secondary);
  font-family: var(--font-body);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.rating-option:hover {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.rating-option.checked {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.rating-option:focus-visible {
  outline-offset: -2px;
}

.option-mark {
  width: 6px;
  height: 6px;
  border-radius: 50%;
}

.rating-option.checked .option-mark {
  background: var(--color-accent);
  box-shadow: 0 0 6px rgba(196, 162, 78, 0.5);
}

.option-body {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

.option-stars {
  display: inline-flex;
  gap: 2px;
}

.option-star {
  width: 12px;
  height: 12px;
  color: var(--color-border-hover);
  flex-shrink: 0;
}

.option-star.filled {
  color: var(--color-accent);
}

.option-star.hollow {
  color: var(--color-text-muted);
}

.option-suffix {
  font-size: 11px;
  color: var(--color-text-muted);
}

.option-count,
.media-count {
  font-size: 11px;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}

.rating-option.empty .option-count {
  opacity: 0.5;
}

/* Media: a segmented toggle in the header's own style. */
.media-toggle {
  display: flex;
  margin: 0 2px;
  background: var(--color-bg);
  border-radius: var(--radius-sm);
  padding: 2px;
  border: 1px solid var(--color-border-subtle);
}

.toggle-btn {
  flex: 1;
  justify-content: center;
  gap: 5px;
  background: none;
  border: none;
  color: var(--color-text-muted);
  padding: 5px 7px;
  border-radius: 4px;
  cursor: pointer;
  display: flex;
  align-items: center;
  font-family: var(--font-body);
  font-size: 11px;
  font-weight: 500;
  transition: all var(--transition-fast);
}

.toggle-btn.active {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.toggle-btn:hover:not(.active) {
  color: var(--color-text-secondary);
}

.toggle-btn:focus-visible {
  outline-offset: -2px;
}

.toggle-btn .media-count {
  font-size: 10px;
}

.filter-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 8px;
  padding: 6px 2px 0 2px;
  border-top: 1px solid var(--color-border-subtle);
}

.footer-clear {
  background: none;
  border: none;
  padding: 4px 6px;
  border-radius: 4px;
  color: var(--color-text-secondary);
  font-family: var(--font-body);
  font-size: 11px;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.footer-clear:hover:not(:disabled) {
  color: var(--color-accent);
  background: var(--color-accent-dim);
}

.footer-clear:disabled {
  color: var(--color-text-muted);
  opacity: 0.6;
  cursor: default;
}

.footer-hint {
  margin-right: 4px;
  padding: 1px 5px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 3px;
  font-family: var(--font-body);
  font-size: 10px;
  font-weight: 600;
  color: var(--color-text-muted);
}

.filter-pop-enter-active {
  transition: opacity 0.14s var(--ease-out), transform 0.14s var(--ease-out);
}

.filter-pop-leave-active {
  transition: opacity 0.1s ease;
}

.filter-pop-enter-from {
  opacity: 0;
  transform: translateY(-4px);
}

.filter-pop-leave-to {
  opacity: 0;
}
</style>
