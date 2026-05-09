<script setup lang="ts">
import { computed } from "vue";
import { useGalleryStore } from "@/stores/gallery";
import { fileUrl } from "@/lib/commands";
import { useHoverPreload } from "@/composables/useHoverPreload";
import { useViewTransition } from "@/composables/useViewTransition";
import type { ImagePair } from "@/types";
import StarRating from "@/components/StarRating.vue";

const props = defineProps<{
  image: ImagePair;
  index: number;
}>();

const store = useGalleryStore();
const { startPreload, cancelPreload } = useHoverPreload();
const { activeTransitionId, startTransition } = useViewTransition();

function onMouseEnter() {
  // Only preload for non-PTP images (PTP requires camera download)
  if (!props.image.hif_path.startsWith("ptp://")) {
    startPreload(props.image.hif_path);
  }
}

const rating = computed(() => store.ratings.get(props.image.id) ?? 0);
const marked = computed(() => store.markedForCompare.has(props.image.id));

const thumbnailSrc = computed(() => {
  // Try thumbnail from cache first (for RAF-based thumbs)
  const thumbPath = store.thumbnailPaths.get(props.image.id);
  if (thumbPath) {
    return fileUrl(thumbPath);
  }
  // For PTP images, check the PTP preview cache
  if (props.image.hif_path.startsWith("ptp://")) {
    const ptpCached = store.ptpPreviewCache.get(props.image.id);
    if (ptpCached) {
      return fileUrl(ptpCached);
    }
    // PTP images without cache: show placeholder (thumbnails generated during catalog)
    return "";
  }
  // Fall back to HIF via asset protocol
  return fileUrl(props.image.hif_path);
});

const borderClass = computed(() => {
  const r = rating.value;
  if (r === 0) return "border-none";
  if (r <= 3) return "border-heif";
  return "border-heif-raw";
});

const badgeText = computed(() => {
  const r = rating.value;
  if (r === 0) return null;
  return "\u2605".repeat(r);
});

const badgeClass = computed(() => {
  const r = rating.value;
  if (r <= 3) return "badge-heifonly";
  return "badge-heifandraw";
});

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function openInViewer(e: MouseEvent) {
  // Find this card's .thumbnail-container for imperative view-transition-name tagging
  const card = (e.currentTarget as HTMLElement);
  const container = card?.querySelector<HTMLElement>(".thumbnail-container");
  startTransition(props.image.id, () => {
    store.currentIndex = props.index;
    store.viewMode = "single";
  }, container);
}
</script>

<template>
  <div :class="['image-card', borderClass]" @click="openInViewer" @mouseenter="onMouseEnter" @mouseleave="cancelPreload">
    <div class="thumbnail-container" :style="{ viewTransitionName: activeTransitionId === image.id ? 'hero-image' : 'none' }">
      <img :src="thumbnailSrc" :alt="image.id" class="thumbnail" loading="lazy" />

      <!-- Compare mark pip: opposite corner from the star badge -->
      <div class="compare-pip" v-if="marked" title="Marked for comparison (M to unmark)">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="20 6 9 17 4 12" />
        </svg>
      </div>

      <!-- Import badge derived from rating -->
      <div class="badge-wrapper" v-if="badgeText">
        <span class="selection-badge" :class="badgeClass">
          {{ badgeText }}
        </span>
      </div>

      <!-- Hover overlay with zoom hint -->
      <div class="hover-overlay">
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.9">
          <circle cx="11" cy="11" r="8" />
          <line x1="21" y1="21" x2="16.65" y2="16.65" />
          <line x1="11" y1="8" x2="11" y2="14" />
          <line x1="8" y1="11" x2="14" y2="11" />
        </svg>
      </div>
    </div>

    <div class="card-footer">
      <div class="card-info">
        <span class="card-name">{{ image.id }}</span>
        <span class="card-size">
          {{ formatSize(image.hif_size) }}
          <template v-if="image.raf_size"> + {{ formatSize(image.raf_size) }}</template>
        </span>
      </div>
      <StarRating :rating="rating" size="sm" readonly />
    </div>
  </div>
</template>

<style scoped>
.image-card {
  background: var(--color-surface);
  border-radius: var(--radius-md);
  overflow: hidden;
  cursor: pointer;
  border: 2px solid transparent;
  transition: all var(--transition-medium);
}

.image-card:hover {
  border-color: var(--color-border-hover);
  transform: translateY(-2px);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
}

.border-heif {
  border-color: var(--color-heif);
}

.border-heif-raw {
  border-color: var(--color-heif-raw);
}

/* Thumbnail */
.thumbnail-container {
  position: relative;
  aspect-ratio: 3 / 2;
  overflow: hidden;
  background: #111;
}

.thumbnail {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform var(--transition-slow);
}

.image-card:hover .thumbnail {
  transform: scale(1.03);
}

/* Hover overlay */
.hover-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(13, 12, 10, 0.4);
  color: white;
  opacity: 0;
  transition: opacity var(--transition-medium);
}

.image-card:hover .hover-overlay {
  opacity: 1;
}

/* Compare mark pip (top-left, opposite the star badge) */
.compare-pip {
  position: absolute;
  top: 8px;
  left: 8px;
  z-index: 2;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--color-accent);
  color: #0d0c0a;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.3);
}

/* Selection badge */
.badge-wrapper {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 2;
}

.selection-badge {
  font-size: 10px;
  font-weight: 700;
  font-family: var(--font-body);
  padding: 3px 8px;
  border-radius: 4px;
  letter-spacing: 0.04em;
  backdrop-filter: blur(4px);
}

.badge-heifonly {
  background: rgba(61, 148, 101, 0.85);
  color: white;
}

.badge-heifandraw {
  background: rgba(72, 120, 184, 0.85);
  color: white;
}

/* Card footer */
.card-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 10px;
  gap: 8px;
}

.card-info {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.card-name {
  font-size: 11.5px;
  font-weight: 500;
  color: var(--color-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.card-size {
  font-size: 10px;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}
</style>
