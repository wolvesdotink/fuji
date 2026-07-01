<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useGalleryStore } from "@/stores/gallery";
import { fileUrl } from "@/lib/commands";
import { useSyncedZoom } from "@/composables/useSyncedZoom";
import StarRating from "@/components/StarRating.vue";
import type { ImagePair } from "@/types";

const store = useGalleryStore();
const {
  transform: zoomTransform,
  transformOrigin: zoomOrigin,
  scale: zoomScale,
  onWheel,
  onMouseDown,
  reset: resetZoom,
} = useSyncedZoom();

// Resolve marked IDs → ImagePair[], preserving the insertion order from
// the Set. Reuse the store's shared id → image map so we don't rebuild a
// lookup over the whole gallery on every marked-set change.
const panes = computed<ImagePair[]>(() => {
  const byId = store.imageById;
  const out: ImagePair[] = [];
  for (const id of store.markedForCompare) {
    const img = byId.get(id);
    if (img) out.push(img);
  }
  return out;
});

// Column layout:
//   1 → 1 pane full-width
//   2 → side by side
//   3 → single row
//   4 → 2×2
//   5–9 → 3 wide (fills 2 or 3 rows)
//   10+ → sqrt layout (closest to square)
const cols = computed(() => {
  const n = panes.value.length;
  if (n <= 3) return n;
  if (n === 4) return 2;
  if (n <= 9) return 3;
  return Math.ceil(Math.sqrt(n));
});

// PTP images need to be downloaded before they can be displayed. We
// kick off downloads in parallel for every marked PTP image as soon as
// the component mounts / the marked set changes. Track in-flight to
// avoid double-downloading the same file.
const ptpLocalPaths = ref<Map<string, string>>(new Map());
const ptpLoading = ref<Set<string>>(new Set());

watch(
  panes,
  async (newPanes) => {
    if (!store.isPtp()) return;
    for (const img of newPanes) {
      if (!img.hif_path.startsWith("ptp://")) continue;
      if (ptpLocalPaths.value.has(img.id)) continue;
      if (ptpLoading.value.has(img.id)) continue;

      const loading = new Set(ptpLoading.value);
      loading.add(img.id);
      ptpLoading.value = loading;

      store
        .ensurePtpPreview(img.id, img.hif_path)
        .then((localPath) => {
          const next = new Map(ptpLocalPaths.value);
          next.set(img.id, localPath);
          ptpLocalPaths.value = next;
        })
        .catch((e) => {
          console.error("PTP download failed for compare pane:", img.id, e);
        })
        .finally(() => {
          const after = new Set(ptpLoading.value);
          after.delete(img.id);
          ptpLoading.value = after;
        });
    }
  },
  { immediate: true }
);

function fullResSrc(img: ImagePair): string {
  if (img.hif_path.startsWith("ptp://")) {
    const local =
      ptpLocalPaths.value.get(img.id) ?? store.ptpPreviewCache.get(img.id);
    return local ? fileUrl(local) : "";
  }
  return fileUrl(img.hif_path);
}

function thumbnailSrc(img: ImagePair): string {
  const thumbPath = store.thumbnailPaths.get(img.id);
  if (thumbPath) return fileUrl(thumbPath);
  if (img.hif_path.startsWith("ptp://")) {
    const cached = store.ptpPreviewCache.get(img.id);
    if (cached) return fileUrl(cached);
    return "";
  }
  return fileUrl(img.hif_path);
}

function ratingFor(img: ImagePair): number {
  return store.ratings.get(img.id) ?? 0;
}

function setRatingFor(img: ImagePair, r: number) {
  // Toggle-off semantics match StarRating's "click same star to clear",
  // and we intentionally do NOT call rateAndAdvance here — in compare
  // mode the user is rating panes independently, not walking a queue.
  store.setRating(img.id, r);
}

function unmark(img: ImagePair) {
  store.toggleMarkForCompare(img.id);
  // Drop below 2 panes → compare loses its meaning. Return to single
  // view, which re-shows whatever image was focused before entering.
  if (store.markedForCompare.size < 2) {
    store.viewMode = "single";
  }
}

function clearAllAndExit() {
  store.clearMarkedForCompare();
  store.viewMode = "single";
}
</script>

<template>
  <div class="image-compare">
    <div class="compare-topbar">
      <div class="topbar-left">
        <span class="compare-label">
          Comparing <strong>{{ panes.length }}</strong> images
        </span>
        <span v-if="zoomScale > 1" class="zoom-readout">
          {{ zoomScale.toFixed(1) }}&times;
        </span>
      </div>
      <div class="topbar-right">
        <button
          class="topbar-btn"
          @click="resetZoom"
          :disabled="zoomScale === 1"
          title="Reset zoom (double-click any pane)"
        >
          Reset zoom
        </button>
        <button
          class="topbar-btn topbar-btn-danger"
          @click="clearAllAndExit"
          title="Clear all marks and exit"
        >
          Clear all
        </button>
        <span class="topbar-hints">
          <kbd class="key-hint-inline">scroll</kbd> zoom
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">drag</kbd> pan
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">dbl-click</kbd> reset
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">Esc</kbd> close
        </span>
      </div>
    </div>

    <div class="compare-grid" :style="{ '--cols': cols }">
      <div
        v-for="img in panes"
        :key="img.id"
        class="compare-pane"
      >
        <div
          class="pane-viewport"
          :class="{ 'is-zoomed': zoomScale > 1 }"
          @wheel="onWheel($event, $event.currentTarget as HTMLElement)"
          @mousedown="onMouseDown($event, $event.currentTarget as HTMLElement)"
          @dblclick="resetZoom"
        >
          <!-- Thumbnail placeholder for instant paint; full-res fades over -->
          <img
            v-if="thumbnailSrc(img)"
            :src="thumbnailSrc(img)"
            :alt="img.id"
            class="pane-img pane-thumb"
            :style="{ transform: zoomTransform, transformOrigin: zoomOrigin }"
            draggable="false"
          />
          <img
            v-if="fullResSrc(img)"
            :src="fullResSrc(img)"
            :alt="img.id"
            class="pane-img pane-full"
            :style="{ transform: zoomTransform, transformOrigin: zoomOrigin }"
            draggable="false"
          />

          <!-- PTP download spinner if we don't even have a thumb yet -->
          <div
            v-if="ptpLoading.has(img.id) && !thumbnailSrc(img) && !fullResSrc(img)"
            class="pane-loading"
          >
            <div class="loading-spinner"></div>
            <span>Downloading&hellip;</span>
          </div>

          <button
            class="pane-unmark"
            @click.stop="unmark(img)"
            @mousedown.stop
            title="Remove from comparison"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        </div>

        <div class="pane-footer">
          <span class="pane-name" :title="img.id">{{ img.id }}</span>
          <StarRating
            :rating="ratingFor(img)"
            size="sm"
            @update:rating="setRatingFor(img, $event)"
          />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.image-compare {
  height: 100%;
  display: flex;
  flex-direction: column;
  background: #050504;
}

/* Top bar */
.compare-topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 16px;
  background: var(--color-surface);
  border-bottom: 1px solid var(--color-border);
  flex-shrink: 0;
  gap: 16px;
}

.topbar-left {
  display: flex;
  align-items: center;
  gap: 10px;
}

.compare-label {
  font-size: 12px;
  color: var(--color-text-secondary);
  letter-spacing: 0.02em;
}

.compare-label strong {
  color: var(--color-text);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.zoom-readout {
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  color: var(--color-accent);
  padding: 2px 8px;
  border-radius: 100px;
  background: rgba(196, 162, 78, 0.12);
  border: 1px solid rgba(196, 162, 78, 0.25);
}

.topbar-right {
  display: flex;
  align-items: center;
  gap: 10px;
}

.topbar-btn {
  background: var(--color-bg);
  color: var(--color-text-secondary);
  border: 1px solid var(--color-border);
  padding: 4px 10px;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 11px;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.topbar-btn:hover:not(:disabled) {
  border-color: var(--color-border-hover);
  color: var(--color-text);
}

.topbar-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.topbar-btn-danger:hover:not(:disabled) {
  border-color: rgba(200, 90, 90, 0.5);
  color: rgb(220, 130, 130);
}

.topbar-hints {
  font-size: 10px;
  color: var(--color-text-muted);
  display: flex;
  align-items: center;
  gap: 4px;
}

.key-hint-inline {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 1px 5px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 3px;
  font-family: var(--font-body);
  font-size: 10px;
  font-weight: 600;
  color: var(--color-text-muted);
}

.hint-sep {
  color: var(--color-border);
}

/* Grid */
.compare-grid {
  flex: 1;
  display: grid;
  grid-template-columns: repeat(var(--cols), 1fr);
  gap: 2px;
  padding: 2px;
  overflow: hidden;
  min-height: 0;
}

.compare-pane {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--color-surface);
  border-radius: var(--radius-sm);
  overflow: hidden;
}

.pane-viewport {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  background: #050504;
  cursor: grab;
}

.pane-viewport.is-zoomed {
  cursor: grab;
}

.pane-viewport.is-zoomed:active {
  cursor: grabbing;
}

.pane-img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: contain;
  /* Don't animate transform — zoom should feel instantaneous, not slidey */
  user-select: none;
  -webkit-user-drag: none;
}

.pane-thumb {
  z-index: 1;
}

.pane-full {
  z-index: 2;
}

.pane-loading {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--color-text-muted);
  font-size: 11px;
  z-index: 3;
}

.loading-spinner {
  width: 24px;
  height: 24px;
  border: 2px solid var(--color-border);
  border-top-color: var(--color-text-secondary);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

/* Per-pane unmark button — sits above the image, doesn't steal zoom input */
.pane-unmark {
  position: absolute;
  top: 6px;
  right: 6px;
  z-index: 4;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(13, 12, 10, 0.75);
  backdrop-filter: blur(6px);
  color: var(--color-text-muted);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  opacity: 0;
  transition: opacity var(--transition-fast), color var(--transition-fast), background var(--transition-fast);
}

.compare-pane:hover .pane-unmark {
  opacity: 1;
}

.pane-unmark:hover {
  color: var(--color-text);
  background: rgba(200, 90, 90, 0.6);
  border-color: rgba(220, 130, 130, 0.4);
}

/* Footer */
.pane-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 5px 10px;
  background: var(--color-surface);
  flex-shrink: 0;
}

.pane-name {
  font-size: 11px;
  color: var(--color-text-secondary);
  font-family: var(--font-body);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}
</style>
