<script setup lang="ts">
import { computed, ref } from "vue";
import { useGalleryStore } from "@/stores/gallery";
import ImageCard from "./ImageCard.vue";
import VirtualGrid, { type VirtualGridHandle } from "./VirtualGrid.vue";
import type { ImagePair } from "@/types";

const store = useGalleryStore();
const imageKey = (image: ImagePair) => image.id;
// Unique per file across catalog reloads (ids are stems and can repeat).
const anchorKey = (image: ImagePair) => image.hif_path;

const grid = ref<VirtualGridHandle | null>(null);
// Forwarded for the keyboard cursor; see VirtualGridHandle (wait a tick after
// scrollToIndex() before querying the newly mounted cards).
defineExpose({
  scrollToIndex: (index: number, block?: "nearest" | "center") =>
    grid.value?.scrollToIndex(index, block),
  columns: computed(() => grid.value?.columns ?? 1),
});
</script>

<template>
  <div class="image-grid-container">
    <div class="loading-bar" v-if="store.isLoadingThumbnails">
      <div
        class="loading-progress"
        :style="{ width: `${(store.thumbnailProgress.completed / Math.max(store.thumbnailProgress.total, 1)) * 100}%` }"
      ></div>
    </div>

    <VirtualGrid
      ref="grid"
      class="image-grid"
      :items="store.images"
      :item-key="imageKey"
      :anchor-key="anchorKey"
    >
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
