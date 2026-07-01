<script setup lang="ts">
import { computed } from "vue";
import { useLibraryStore } from "@/stores/library";
import { fileUrl } from "@/lib/commands";
import { useHoverPreload } from "@/composables/useHoverPreload";
import { useViewTransition } from "@/composables/useViewTransition";
import type { LibraryImage } from "@/types";
import StarRating from "@/components/StarRating.vue";

const props = defineProps<{
  image: LibraryImage;
  index: number;
}>();

const store = useLibraryStore();
const { startPreload, cancelPreload } = useHoverPreload();
const { activeTransitionId, startTransition } = useViewTransition();

const rating = computed(() => store.getRating(props.image.file_path));

const hasThumbnail = computed(() => store.thumbnailPaths.has(props.image.id));

const thumbnailSrc = computed(() => {
  const thumbPath = store.thumbnailPaths.get(props.image.id);
  if (thumbPath) {
    return fileUrl(thumbPath);
  }
  return null;
});

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
  });
}

function openInViewer(e: MouseEvent) {
  const card = (e.currentTarget as HTMLElement);
  const container = card?.querySelector<HTMLElement>(".thumbnail-container");
  startTransition(props.image.id, () => {
    store.currentIndex = props.index;
    store.viewMode = "single";
  }, container);
}
</script>

<template>
  <div class="library-card" @click="openInViewer" @mouseenter="startPreload(image.file_path)" @mouseleave="cancelPreload">
    <div class="thumbnail-container" :style="{ viewTransitionName: activeTransitionId === image.id ? 'hero-image' : 'none' }">
      <!-- Shimmer skeleton while thumbnail generates -->
      <div v-if="!hasThumbnail" class="thumbnail-skeleton">
        <div class="skeleton-shimmer"></div>
      </div>

      <!-- Actual thumbnail with fade-in -->
      <img
        v-else
        :src="thumbnailSrc!"
        :alt="image.id"
        class="thumbnail"
        loading="lazy"
      />

      <!-- Hover overlay -->
      <div class="hover-overlay" v-if="hasThumbnail">
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
        <span class="card-meta">{{ formatSize(image.file_size) }} &middot; {{ formatDate(image.date_modified) }}</span>
      </div>
      <StarRating :rating="rating" size="sm" readonly />
    </div>
  </div>
</template>

<style scoped>
.library-card {
  background: var(--color-surface);
  border-radius: var(--radius-md);
  overflow: hidden;
  cursor: pointer;
  border: 2px solid transparent;
  transition: all var(--transition-medium);
  /* Skip layout/paint for off-screen cards. The intrinsic-size hint keeps
     the scrollbar stable so scroll-restore lands. No-op below macOS 13. */
  content-visibility: auto;
  contain-intrinsic-size: auto 280px;
}

.library-card:hover {
  border-color: var(--color-border-hover);
  transform: translateY(-2px);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
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
  animation: thumb-fade-in 0.3s ease both;
}

/* Skeleton shimmer placeholder */
.thumbnail-skeleton {
  position: absolute;
  inset: 0;
  background: var(--color-surface);
  overflow: hidden;
}

.skeleton-shimmer {
  position: absolute;
  inset: 0;
  background: linear-gradient(
    105deg,
    transparent 30%,
    rgba(196, 162, 78, 0.04) 45%,
    rgba(196, 162, 78, 0.06) 50%,
    rgba(196, 162, 78, 0.04) 55%,
    transparent 70%
  );
  animation: shimmer 2s ease-in-out infinite;
}

@keyframes shimmer {
  0% { transform: translateX(-100%); }
  100% { transform: translateX(100%); }
}

@keyframes thumb-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

.library-card:hover .thumbnail {
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

.library-card:hover .hover-overlay {
  opacity: 1;
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

.card-meta {
  font-size: 10px;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}
</style>
