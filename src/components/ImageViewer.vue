<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useGalleryStore } from "@/stores/gallery";
import { fileUrl } from "@/lib/commands";
import { decodeAhead } from "@/composables/useHoverPreload";
import StarRating from "@/components/StarRating.vue";

const store = useGalleryStore();

const image = computed(() => store.currentImage);
const rating = computed(() =>
  image.value ? (store.ratings.get(image.value.id) ?? 0) : 0
);
const isMarked = computed(() =>
  image.value ? store.markedForCompare.has(image.value.id) : false
);
const markedCount = computed(() => store.markedForCompare.size);

// For PTP images, we need to download the HIF before displaying
const ptpLocalPath = ref<string | null>(null);
const ptpLoading = ref(false);

watch(
  () => image.value?.id,
  async () => {
    if (!image.value) return;
    if (store.isPtp() && image.value.hif_path.startsWith("ptp://")) {
      ptpLoading.value = true;
      ptpLocalPath.value = null;
      try {
        const localPath = await store.ensurePtpPreview(
          image.value.id,
          image.value.hif_path
        );
        ptpLocalPath.value = localPath;
      } catch (e) {
        console.error("Failed to download PTP preview:", e);
      } finally {
        ptpLoading.value = false;
      }
    } else {
      ptpLocalPath.value = null;
    }
  },
  { immediate: true }
);

const imageSrc = computed(() => {
  if (!image.value) return "";
  // PTP: use downloaded local path
  if (ptpLocalPath.value) {
    return fileUrl(ptpLocalPath.value);
  }
  // Mass storage: use direct path
  if (!image.value.hif_path.startsWith("ptp://")) {
    return fileUrl(image.value.hif_path);
  }
  return ""; // PTP image still loading
});

// Progressive loading: show a cached thumbnail instantly, crossfade to
// full-res. When no thumbnail is cached we return "" rather than the
// full-res HIF — decoding a multi-MB HIF just to use it as its own
// placeholder stalls the swap it's meant to hide.
const thumbnailSrc = computed(() => {
  if (!image.value) return "";
  const thumbPath = store.thumbnailPaths.get(image.value.id);
  if (thumbPath) return fileUrl(thumbPath);
  if (image.value.hif_path.startsWith("ptp://")) {
    const ptpCached = store.ptpPreviewCache.get(image.value.id);
    if (ptpCached) return fileUrl(ptpCached);
  }
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
    const imgs = store.images;
    const ptp = store.isPtp();
    for (const offset of [-1, 1, 2]) {
      const adjIdx = newIdx + offset;
      if (adjIdx < 0 || adjIdx >= imgs.length) continue;
      const adjImage = imgs[adjIdx];
      if (adjImage.hif_path.startsWith("ptp://")) {
        // PTP neighbors must be downloaded from the camera first. Limit to
        // the immediate neighbors [-1, 1] to bound camera bandwidth;
        // ensurePtpPreview dedups in-flight + cached requests.
        if (ptp && (offset === -1 || offset === 1)) {
          store
            .ensurePtpPreview(adjImage.id, adjImage.hif_path)
            .then((localPath) => decodeAhead(fileUrl(localPath)))
            .catch(() => {});
        }
      } else {
        decodeAhead(fileUrl(adjImage.hif_path));
      }
    }
  },
  { immediate: true }
);

const position = computed(
  () => `${store.currentIndex + 1} / ${store.images.length}`
);

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function onRating(r: number) {
  if (image.value) {
    store.rateAndAdvance(r);
  }
}
</script>

<template>
  <div class="image-viewer" v-if="image">
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
      :disabled="store.currentIndex === store.images.length - 1"
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

      <!-- PTP loading indicator (shown over thumbnail) -->
      <div v-if="ptpLoading && !thumbnailSrc" class="loading-indicator">
        <div class="loading-spinner"></div>
        <span>Downloading from camera...</span>
      </div>

      <!-- Full-res image (fades in over thumbnail) -->
      <img
        v-if="imageSrc"
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
        <span class="filename">{{ image.id }}</span>
        <span v-if="isMarked" class="marked-pill" title="Marked for comparison">
          <span class="marked-dot"></span>marked
        </span>
        <span class="bar-sep">&middot;</span>
        <span class="sizes">
          HIF {{ formatSize(image.hif_size) }}
          <template v-if="image.raf_size">
            &middot; RAF {{ formatSize(image.raf_size) }}
          </template>
        </span>
      </div>

      <div class="bar-rating">
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
          <kbd class="key-hint-inline">Space</kbd> next unrated
          <span class="hint-sep">&middot;</span>
          <kbd class="key-hint-inline">M</kbd> mark
          <template v-if="markedCount >= 2">
            <span class="hint-sep">&middot;</span>
            <kbd class="key-hint-inline">C</kbd> compare ({{ markedCount }})
          </template>
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.image-viewer {
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

/* Subtle vignette for darkroom feel */
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

.image-viewer:hover .nav-btn:not(:disabled) {
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

.marked-pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--color-accent);
  padding: 2px 8px;
  border-radius: 100px;
  background: rgba(196, 162, 78, 0.1);
  border: 1px solid rgba(196, 162, 78, 0.3);
  white-space: nowrap;
}

.marked-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--color-accent);
  box-shadow: 0 0 5px rgba(196, 162, 78, 0.5);
}

/* Rating section */
.bar-rating {
  display: flex;
  align-items: center;
  gap: 10px;
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

/* Loading indicator for PTP downloads */
.loading-indicator {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  color: var(--color-text-muted);
  font-size: 13px;
}

.loading-spinner {
  width: 32px;
  height: 32px;
  border: 2px solid var(--color-border);
  border-top-color: var(--color-text-secondary);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
</style>
