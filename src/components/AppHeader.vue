<script setup lang="ts">
import { computed } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { useGalleryStore } from "@/stores/gallery";
import { useAppStore } from "@/stores/app";

const store = useGalleryStore();
const appStore = useAppStore();

const summary = computed(() => store.selectionSummary);

const reviewPercent = computed(() => {
  const total = store.images.length;
  if (total === 0) return 0;
  return Math.round((summary.value.reviewed / total) * 100);
});

function segmentStatus(imageId: string): string {
  const rating = store.ratings.get(imageId);
  if (!rating || rating === 0) return "seg-unreviewed";
  if (rating <= 3) return "seg-heif";
  return "seg-heif-raw";
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

async function pickDestination() {
  const dir = await open({
    directory: true,
    title: "Choose import destination",
    defaultPath: appStore.destinationPath || "/Volumes/LaCie",
  });

  if (dir) {
    store.importDestination = dir as string;
    appStore.setDestination(dir as string);
  }
}

function backToLibrary() {
  appStore.switchToLibrary();
}

function jumpTo(index: number) {
  store.currentIndex = index;
  if (store.viewMode === "grid") {
    store.viewMode = "single";
  }
}

function openCompare() {
  if (store.viewMode === "compare") {
    store.viewMode = "single";
  } else {
    store.openCompareView();
  }
}
</script>

<template>
  <header class="app-header">
    <div class="header-main">
      <div class="header-left">
        <button class="back-button" @click="backToLibrary" title="Back to Library">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="15 18 9 12 15 6" />
          </svg>
          Library
        </button>

        <div class="camera-info">
          <span class="camera-dot"></span>
          <span class="camera-name">{{ store.camera?.name }}</span>
        </div>
        <div class="header-meta">
          <span class="image-count">{{ store.images.length }} images</span>
          <span class="meta-sep">&middot;</span>
          <span class="review-stat">{{ reviewPercent }}% reviewed</span>
        </div>
      </div>

      <div class="header-center">
        <div class="selection-pills">
          <span class="pill pill-remaining" v-if="summary.remaining > 0">
            {{ summary.remaining }} unrated
          </span>
          <span class="pill pill-skip" v-if="summary.skip - summary.remaining > 0">
            {{ summary.skip - summary.remaining }} skip
          </span>
          <span class="pill pill-heif" v-if="summary.heifOnly > 0">
            {{ summary.heifOnly }} 1&ndash;3&starf;
          </span>
          <span class="pill pill-heif-raw" v-if="summary.heifAndRaw > 0">
            {{ summary.heifAndRaw }} 4&ndash;5&starf;
          </span>
        </div>
      </div>

      <div class="header-right">
        <div class="view-toggle">
          <button
            :class="['toggle-btn', { active: store.viewMode === 'single' }]"
            @click="store.viewMode = 'single'"
            title="Single view (G)"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="3" />
            </svg>
          </button>
          <button
            :class="['toggle-btn', { active: store.viewMode === 'grid' }]"
            @click="store.viewMode = 'grid'"
            title="Grid view (G)"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="7" height="7" rx="1.5" />
              <rect x="14" y="3" width="7" height="7" rx="1.5" />
              <rect x="3" y="14" width="7" height="7" rx="1.5" />
              <rect x="14" y="14" width="7" height="7" rx="1.5" />
            </svg>
          </button>
        </div>

        <button
          class="compare-btn"
          :class="{ active: store.viewMode === 'compare' }"
          :disabled="store.markedForCompare.size < 2"
          @click="openCompare"
          :title="store.markedForCompare.size < 2 ? 'Mark 2+ images with M to compare' : `Compare ${store.markedForCompare.size} marked images (C)`"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="4" width="8" height="16" rx="1.5" />
            <rect x="13" y="4" width="8" height="16" rx="1.5" />
          </svg>
          <span>Compare</span>
          <span v-if="store.markedForCompare.size > 0" class="compare-count">{{ store.markedForCompare.size }}</span>
        </button>

        <button class="dest-button" @click="pickDestination">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
          {{ store.importDestination ? '\u2026' + store.importDestination.split('/').slice(-2).join('/') : 'Choose folder' }}
        </button>

        <button
          class="import-button"
          :disabled="!store.canImport"
          @click="store.startImport()"
        >
          Import {{ summary.toImport }}
          <span class="import-size">{{ formatBytes(store.totalImportSize) }}</span>
        </button>
      </div>
    </div>

    <!-- Review minimap -->
    <div class="review-minimap" v-if="store.images.length > 0">
      <div
        v-for="(img, i) in store.images"
        :key="img.id"
        :class="['minimap-seg', segmentStatus(img.id), { current: i === store.currentIndex && store.viewMode === 'single' }]"
        @click="jumpTo(i)"
      />
    </div>
  </header>
</template>

<style scoped>
.app-header {
  flex-shrink: 0;
  background: var(--color-surface);
  border-bottom: 1px solid var(--color-border);
}

.header-main {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 20px;
  gap: 16px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 14px;
}

.back-button {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: none;
  border: 1px solid var(--color-border-subtle);
  color: var(--color-text-muted);
  padding: 4px 10px;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 11px;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.back-button:hover {
  border-color: var(--color-border-hover);
  color: var(--color-text-secondary);
  background: var(--color-surface-hover);
}

.camera-info {
  display: flex;
  align-items: center;
  gap: 8px;
}

.camera-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--color-heif);
  box-shadow: 0 0 8px rgba(61, 148, 101, 0.4);
  flex-shrink: 0;
}

.camera-name {
  font-weight: 600;
  font-size: 13px;
  color: var(--color-text);
  white-space: nowrap;
}

.header-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--color-text-muted);
}

.meta-sep {
  color: var(--color-border);
}

.review-stat {
  color: var(--color-text-secondary);
  font-variant-numeric: tabular-nums;
}

.header-center {
  flex: 1;
  display: flex;
  justify-content: center;
  min-width: 0;
}

.selection-pills {
  display: flex;
  gap: 5px;
}

.pill {
  font-size: 11px;
  padding: 3px 10px;
  border-radius: 100px;
  font-weight: 500;
  white-space: nowrap;
  letter-spacing: 0.01em;
  transition: all var(--transition-fast);
}

.pill-remaining {
  background: var(--color-border-subtle);
  color: var(--color-text-secondary);
}

.pill-skip {
  background: var(--color-skip-bg);
  color: var(--color-skip-light);
}

.pill-heif {
  background: var(--color-heif-bg);
  color: var(--color-heif-light);
}

.pill-heif-raw {
  background: var(--color-heif-raw-bg);
  color: var(--color-heif-raw-light);
}

.header-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.view-toggle {
  display: flex;
  background: var(--color-bg);
  border-radius: var(--radius-sm);
  padding: 2px;
  border: 1px solid var(--color-border-subtle);
}

.toggle-btn {
  background: none;
  border: none;
  color: var(--color-text-muted);
  padding: 5px 7px;
  border-radius: 4px;
  cursor: pointer;
  display: flex;
  align-items: center;
  transition: all var(--transition-fast);
}

.toggle-btn.active {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.toggle-btn:hover:not(.active) {
  color: var(--color-text-secondary);
}

.compare-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--color-bg);
  color: var(--color-text-muted);
  border: 1px solid var(--color-border);
  padding: 6px 10px;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 12px;
  cursor: pointer;
  transition: all var(--transition-fast);
  white-space: nowrap;
}

.compare-btn:hover:not(:disabled) {
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.compare-btn.active {
  background: var(--color-accent);
  color: #0d0c0a;
  border-color: var(--color-accent);
}

.compare-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.compare-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  border-radius: 100px;
  background: var(--color-accent);
  color: #0d0c0a;
  font-size: 10px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.compare-btn.active .compare-count {
  background: #0d0c0a;
  color: var(--color-accent);
}

.dest-button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--color-bg);
  color: var(--color-text-muted);
  border: 1px solid var(--color-border);
  padding: 6px 12px;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 12px;
  cursor: pointer;
  max-width: 170px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: all var(--transition-fast);
}

.dest-button:hover {
  border-color: var(--color-border-hover);
  color: var(--color-text-secondary);
}

.import-button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--color-accent);
  color: #0d0c0a;
  border: none;
  padding: 7px 18px;
  border-radius: var(--radius-md);
  font-family: var(--font-body);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-medium);
  white-space: nowrap;
}

.import-button:hover:not(:disabled) {
  background: var(--color-accent-hover);
  transform: translateY(-1px);
  box-shadow: 0 4px 12px rgba(196, 162, 78, 0.2);
}

.import-button:active:not(:disabled) {
  transform: translateY(0);
}

.import-button:disabled {
  background: var(--color-border);
  color: var(--color-text-muted);
  cursor: not-allowed;
  box-shadow: none;
}

.import-size {
  font-weight: 400;
  opacity: 0.7;
}

/* Review minimap */
.review-minimap {
  display: flex;
  height: 3px;
}

.minimap-seg {
  flex: 1;
  cursor: pointer;
  transition: background var(--transition-fast);
  position: relative;
}

.minimap-seg.seg-unreviewed {
  background: var(--color-border-subtle);
}

.minimap-seg.seg-skip {
  background: var(--color-skip);
  opacity: 0.5;
}

.minimap-seg.seg-heif {
  background: var(--color-heif);
  opacity: 0.8;
}

.minimap-seg.seg-heif-raw {
  background: var(--color-heif-raw);
  opacity: 0.8;
}

.minimap-seg.current {
  background: var(--color-accent) !important;
  opacity: 1 !important;
  box-shadow: 0 0 6px rgba(196, 162, 78, 0.5);
}

.minimap-seg:hover {
  opacity: 1 !important;
  filter: brightness(1.3);
}
</style>
