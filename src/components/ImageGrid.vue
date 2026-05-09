<script setup lang="ts">
import { useGalleryStore } from "@/stores/gallery";
import ImageCard from "./ImageCard.vue";

const store = useGalleryStore();
</script>

<template>
  <div class="image-grid-container">
    <div class="loading-bar" v-if="store.isLoadingThumbnails">
      <div
        class="loading-progress"
        :style="{ width: `${(store.thumbnailProgress.completed / Math.max(store.thumbnailProgress.total, 1)) * 100}%` }"
      ></div>
    </div>

    <div class="image-grid">
      <ImageCard
        v-for="(image, index) in store.images"
        :key="image.id"
        :image="image"
        :index="index"
      />
    </div>
  </div>
</template>

<style scoped>
.image-grid-container {
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

.image-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 14px;
}
</style>
