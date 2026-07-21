<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useLibraryStore } from "@/stores/library";
import { fileUrl } from "@/lib/commands";
import { decodeAhead } from "@/composables/useHoverPreload";
import StarRating from "@/components/StarRating.vue";

const store = useLibraryStore();

const image = computed(() => store.currentImage);

const rating = computed(() =>
  image.value ? store.getRating(image.value.file_path) : 0
);

const imageSrc = computed(() => {
  if (!image.value) return "";
  return fileUrl(image.value.file_path);
});

// Progressive loading: show a cached thumbnail instantly, crossfade to
// full-res. When no thumbnail is cached we return "" rather than the full
// file — decoding the full-res image just to use it as its own placeholder
// stalls the swap it's meant to hide.
const thumbnailSrc = computed(() => {
  if (!image.value) return "";
  const thumbPath = store.thumbnailPaths.get(image.value.id);
  if (thumbPath) return fileUrl(thumbPath);
  return "";
});

const fullResLoaded = ref(false);

// Reset loaded state when navigating to a new image
watch(
  () => image.value?.id,
  () => {
    fullResLoaded.value = false;
  }
);

function onFullResLoad() {
  fullResLoaded.value = true;
}

// Adjacent image decode-ahead: warm prev, next, and next+1 so the swap to
// full-res is instant. decodeAhead runs img.decode() off the nav path and
// retains the decoded bitmap in a shared, bounded LRU.
watch(
  () => store.currentIndex,
  (newIdx) => {
    const imgs = store.displayImages;
    for (const offset of [-1, 1, 2]) {
      const adjIdx = newIdx + offset;
      if (adjIdx >= 0 && adjIdx < imgs.length) {
        if (imgs[adjIdx].media_type === "Image") {
          decodeAhead(fileUrl(imgs[adjIdx].file_path));
        }
      }
    }
  },
  { immediate: true }
);

const position = computed(
  () => `${store.currentIndex + 1} / ${store.displayImages.length}`
);

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatDate(timestamp: number): string {
  if (!timestamp) return "";
  const d = new Date(timestamp * 1000);
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function onRating(r: number) {
  if (image.value) {
    store.setRating(image.value.file_path, r);
  }
}
</script>

<template>
  <div class="library-viewer" v-if="image">
    <!-- Navigation arrows -->
    <button
      class="nav-btn nav-prev"
      @click="store.navigatePrev()"
      :disabled="store.currentIndex === 0"
    >
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="15 18 9 12 15 6" />
      </svg>
    </button>

    <button
      class="nav-btn nav-next"
      @click="store.navigateNext()"
      :disabled="store.currentIndex === store.displayImages.length - 1"
    >
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="9 18 15 12 9 6" />
      </svg>
    </button>

    <!-- Main image area -->
    <div class="image-container" style="view-transition-name: hero-image">
      <!-- Thumbnail placeholder (instant) -->
      <img
        v-if="thumbnailSrc && !fullResLoaded"
        :src="thumbnailSrc"
        :alt="image.id"
        class="preview-image"
        :key="'thumb-' + image.id"
      />

      <video
        v-if="image.media_type === 'Video'"
        :src="imageSrc"
        :poster="thumbnailSrc || undefined"
        :key="'video-' + image.id"
        class="full-video"
        controls
        preload="metadata"
      />

      <!-- Full-res image (fades in over thumbnail) -->
      <img
        v-else-if="imageSrc"
        :src="imageSrc"
        :alt="image.id"
        :class="['full-image', { loaded: fullResLoaded }]"
        :key="'full-' + image.id"
        decoding="async"
        @load="onFullResLoad"
      />

      <div class="vignette"></div>
    </div>

    <!-- Bottom bar -->
    <div class="viewer-bar">
      <div class="bar-left">
        <span class="position">{{ position }}</span>
        <span class="bar-sep">&middot;</span>
        <span class="filename">{{ image.file_name }}</span>
        <span class="bar-sep">&middot;</span>
        <span class="sizes">{{ formatSize(image.file_size) }}</span>
        <span class="bar-sep">&middot;</span>
        <span class="date">{{ formatDate(image.date_modified) }}</span>
      </div>

      <div class="bar-center">
        <StarRating
          :rating="rating"
          @update:rating="onRating"
        />
      </div>

      <div class="bar-right">
        <span class="hint-text">
          <kbd class="key-hint-inline">1-5</kbd> rate
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">0</kbd> clear
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">&larr;&rarr;</kbd> navigate
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">G</kbd> grid
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">Esc</kbd> back
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.library-viewer {
  height: 100%;
  display: flex;
  flex-direction: column;
  position: relative;
}

.image-container {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background: #050504;
  position: relative;
}

/* Thumbnail placeholder — shown instantly */
.preview-image {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  position: absolute;
  inset: 0;
  margin: auto;
}

/* Full-res — fades in over thumbnail */
.full-image {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  position: absolute;
  inset: 0;
  margin: auto;
  opacity: 0;
  transition: opacity 0.3s ease;
}

.full-image.loaded {
  opacity: 1;
}

.full-video {
  max-width: 100%;
  max-height: 100%;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.vignette {
  position: absolute;
  inset: 0;
  pointer-events: none;
  background: radial-gradient(ellipse at center, transparent 60%, rgba(5, 5, 4, 0.35) 100%);
}

/* Navigation buttons */
.nav-btn {
  position: absolute;
  top: 50%;
  transform: translateY(-50%);
  z-index: 10;
  background: rgba(13, 12, 10, 0.7);
  backdrop-filter: blur(8px);
  border: 1px solid rgba(255, 255, 255, 0.06);
  color: var(--color-text-secondary);
  width: 40px;
  height: 40px;
  border-radius: 50%;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all var(--transition-medium);
  opacity: 0;
}

.library-viewer:hover .nav-btn:not(:disabled) {
  opacity: 1;
}

.nav-btn:hover:not(:disabled) {
  background: rgba(13, 12, 10, 0.9);
  color: var(--color-text);
  border-color: rgba(255, 255, 255, 0.1);
  transform: translateY(-50%) scale(1.05);
}

.nav-btn:active:not(:disabled) {
  transform: translateY(-50%) scale(0.95);
}

.nav-btn:disabled {
  opacity: 0 !important;
  cursor: default;
}

.nav-prev {
  left: 16px;
}

.nav-next {
  right: 16px;
}

/* Bottom viewer bar */
.viewer-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 20px;
  background: var(--color-surface);
  border-top: 1px solid var(--color-border);
  flex-shrink: 0;
  gap: 16px;
}

.bar-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.position {
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.bar-sep {
  color: var(--color-border);
  font-size: 10px;
}

.filename {
  font-size: 12px;
  color: var(--color-text-secondary);
  white-space: nowrap;
}

.sizes {
  font-size: 11px;
  color: var(--color-text-muted);
  white-space: nowrap;
}

.date {
  font-size: 11px;
  color: var(--color-text-muted);
  white-space: nowrap;
}

/* Center rating */
.bar-center {
  display: flex;
  align-items: center;
}

/* Right hints */
.bar-right {
  display: flex;
  align-items: center;
}

.hint-text {
  font-size: 11px;
  color: var(--color-text-muted);
  display: flex;
  align-items: center;
  gap: 5px;
  white-space: nowrap;
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
</style>
