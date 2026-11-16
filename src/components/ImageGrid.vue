<script setup lang="ts">
import { useGalleryStore } from "@/stores/gallery";
import ImageCard from "./ImageCard.vue";
import VirtualGrid from "./VirtualGrid.vue";
import type { ImagePair } from "@/types";

const store = useGalleryStore();
const imageKey = (image: ImagePair) => image.id;
</script>

<template>
  <div class="image-grid-container">
    <div class="loading-bar" v-if="store.isLoadingThumbnails">
      <div
        class="loading-progress"
        :style="{ width: `${(store.thumbnailProgress.completed / Math.max(store.thumbnailProgress.total, 1)) * 100}%` }"
      ></div>
    </div>

    <VirtualGrid class="image-grid" :items="store.images" :item-key="imageKey">
      <template #default="{ item, index }">
        <ImageCard :image="item" :index="index" />
      </template>
    </VirtualGrid>
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
</style>
