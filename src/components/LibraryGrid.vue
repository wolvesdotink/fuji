<script setup lang="ts">
import { ref, watch, onUnmounted } from "vue";
import { useLibraryStore } from "@/stores/library";
import LibraryCard from "./LibraryCard.vue";

const store = useLibraryStore();

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

    <!-- Empty state (no images, not loading) -->
    <div class="empty-library" v-if="store.displayImages.length === 0 && !store.isLoading">
      <p class="empty-text" v-if="store.searchQuery">
        No results for "{{ store.searchQuery }}"
      </p>
      <p class="empty-text" v-else>No images found in this folder</p>
    </div>

    <!-- Grid -->
    <div class="library-grid" v-if="store.displayImages.length > 0">
      <LibraryCard
        v-for="(image, index) in store.displayImages"
        :key="image.id"
        :image="image"
        :index="index"
      />
    </div>
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

.library-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 14px;
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
