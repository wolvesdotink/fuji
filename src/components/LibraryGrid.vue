<script setup lang="ts">
import { computed, ref, watch, onUnmounted } from "vue";
import { useLibraryStore } from "@/stores/library";
import LibraryCard from "./LibraryCard.vue";
import VirtualGrid, { type VirtualGridHandle } from "./VirtualGrid.vue";
import { describeFilters } from "@/lib/libraryFilter";
import type { LibraryImage } from "@/types";

const store = useLibraryStore();
const imageKey = (image: LibraryImage) => image.id;
// Unique per file across library reloads (ids are stems and can repeat).
const anchorKey = (image: LibraryImage) => image.file_path;

const grid = ref<VirtualGridHandle | null>(null);
// Forwarded for the keyboard cursor; see VirtualGridHandle (wait a tick after
// scrollToIndex() before querying the newly mounted cards).
defineExpose({
  scrollToIndex: (index: number, block?: "nearest" | "center") =>
    grid.value?.scrollToIndex(index, block),
  columns: computed(() => grid.value?.columns ?? 1),
});

// Delayed show prevents the indicator from flashing on fast loads
const showLoading = ref(false);
let loadingTimeout: number | null = null;

watch(
  () => store.isLoading,
  (loading) => {
    if (loading && store.displayImages.length === 0) {
      loadingTimeout = window.setTimeout(() => {
        showLoading.value = true;
      }, 300);
    } else {
      if (loadingTimeout) clearTimeout(loadingTimeout);
      showLoading.value = false;
    }
  },
  { immediate: true }
);

onUnmounted(() => {
  if (loadingTimeout) clearTimeout(loadingTimeout);
});
</script>

<template>
  <div class="library-grid-container">
    <div class="loading-bar" v-if="store.isLoadingThumbnails">
      <div
        class="loading-progress"
        :style="{ width: `${(store.thumbnailProgress.completed / Math.max(store.thumbnailProgress.total, 1)) * 100}%` }"
      ></div>
    </div>

    <!-- Loading state: aperture indicator while fetching image list & metadata -->
    <Transition name="loading-fade">
      <div
        v-if="showLoading && store.displayImages.length === 0"
        class="library-loading"
        key="loading"
      >
        <div class="aperture-container">
          <div class="aperture-ring"></div>
          <svg
            class="aperture-icon"
            width="48"
            height="48"
            viewBox="0 0 48 48"
            fill="none"
          >
            <path
              v-for="i in 6"
              :key="i"
              class="blade"
              :style="{ animationDelay: `${(i - 1) * 0.5}s` }"
              :transform="`rotate(${(i - 1) * 60} 24 24)`"
              d="M24 10 L27.5 17 L24 19 L20.5 17 Z"
              stroke="currentColor"
              stroke-width="1"
              fill="currentColor"
            />
          </svg>
        </div>
        <p class="loading-label">Loading library...</p>
      </div>
    </Transition>

    <!-- Empty state (no media, not loading) -->
    <div class="empty-library" v-if="store.displayImages.length === 0 && !store.isLoading">
      <!-- Filters hid everything that would otherwise show -->
      <div class="empty-filtered" v-if="store.isFiltered && store.unfilteredCount > 0">
        <svg class="empty-icon" width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true">
          <path d="M3 5h18M6.5 12h11M10 19h4" />
        </svg>
        <p class="empty-title">Nothing matches these filters</p>
        <p class="empty-detail">
          {{ describeFilters(store.ratingFilter, store.mediaFilter) }} in {{ store.unfilteredCount }}
          {{ store.searchQuery ? `results for "${store.searchQuery}"` : "media" }}
        </p>
        <button class="empty-clear" type="button" @click="store.clearFilters()">Clear filters</button>
      </div>
      <p class="empty-text" v-else-if="store.searchQuery">
        No results for "{{ store.searchQuery }}"
      </p>
      <p class="empty-text" v-else>No photos or videos found in this folder</p>
    </div>

    <!-- Grid -->
    <VirtualGrid
      v-if="store.displayImages.length > 0"
      ref="grid"
      class="library-grid"
      :items="store.displayImages"
      :item-key="imageKey"
      :anchor-key="anchorKey"
    >
      <template #default="{ item, index }">
        <LibraryCard :image="item" :index="index" />
      </template>
    </VirtualGrid>
  </div>
</template>

<style scoped>
.library-grid-container {
  height: 100%;
  overflow-y: auto;
  padding: 16px 20px;
}

.loading-bar {
  height: 2px;
  background: var(--color-border-subtle);
  border-radius: 1px;
  margin-bottom: 16px;
  overflow: hidden;
}

.loading-progress {
  height: 100%;
  background: linear-gradient(90deg, var(--color-accent), var(--color-heif-raw));
  transition: width 0.3s ease;
  border-radius: 1px;
}

.empty-library {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 60%;
}

.empty-text {
  font-size: 14px;
  color: var(--color-text-muted);
}

.empty-filtered {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  animation: fadeIn 0.3s ease both;
}

.empty-icon {
  color: var(--color-accent);
  opacity: 0.7;
  margin-bottom: 14px;
}

.empty-title {
  font-size: 15px;
  font-weight: 500;
  color: var(--color-text);
}

.empty-detail {
  margin-top: 6px;
  font-size: 13px;
  color: var(--color-text-muted);
}

.empty-clear {
  margin-top: 18px;
  padding: 7px 16px;
  background: var(--color-accent-dim);
  border: 1px solid rgba(196, 162, 78, 0.35);
  border-radius: var(--radius-sm);
  color: var(--color-accent);
  font-family: var(--font-body);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.empty-clear:hover {
  background: rgba(196, 162, 78, 0.18);
  color: var(--color-accent-hover);
}

/* Loading indicator */
.library-loading {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 60%;
  animation: fadeIn 0.4s ease both;
}

.aperture-container {
  position: relative;
  width: 80px;
  height: 80px;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 20px;
}

.aperture-ring {
  position: absolute;
  width: 80px;
  height: 80px;
  border-radius: 50%;
  border: 1px solid var(--color-border);
  animation: pulse-ring 3s ease-in-out infinite;
}

.aperture-icon {
  color: var(--color-accent);
  animation: aperture-breathe 3s ease-in-out infinite;
}

.blade {
  fill-opacity: 0.08;
  stroke-opacity: 0.15;
  animation: blade-highlight 3s ease-in-out infinite;
}

.loading-label {
  font-size: 13px;
  color: var(--color-text-muted);
  letter-spacing: 0.02em;
  animation: fadeIn 0.6s ease 0.2s both;
}

/* Transition out */
.loading-fade-leave-active {
  transition: all 0.25s var(--ease-out);
}

.loading-fade-leave-to {
  opacity: 0;
  transform: translateY(-8px);
}

/* Aperture animations */
@keyframes aperture-breathe {
  0%, 100% {
    transform: rotate(0deg) scale(1);
  }
  50% {
    transform: rotate(30deg) scale(0.92);
  }
}

@keyframes blade-highlight {
  0%, 100% {
    fill-opacity: 0.08;
    stroke-opacity: 0.15;
  }
  16.67% {
    fill-opacity: 0.25;
    stroke-opacity: 0.6;
  }
}
</style>
