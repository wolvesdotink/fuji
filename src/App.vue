<script setup lang="ts">
import { onMounted, onUnmounted, computed } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useAppStore } from "@/stores/app";
import { useGalleryStore } from "@/stores/gallery";
import { useLibraryStore } from "@/stores/library";
import { useKeyboardNav } from "@/composables/useKeyboardNav";
import type { CameraVolume } from "@/types";
import EmptyState from "@/components/EmptyState.vue";
import AppHeader from "@/components/AppHeader.vue";
import ImageGrid from "@/components/ImageGrid.vue";
import ImageViewer from "@/components/ImageViewer.vue";
import ImageCompare from "@/components/ImageCompare.vue";
import ImportProgress from "@/components/ImportProgress.vue";
import LibraryHeader from "@/components/LibraryHeader.vue";
import LibraryGrid from "@/components/LibraryGrid.vue";
import LibraryViewer from "@/components/LibraryViewer.vue";
import CameraImportPrompt from "@/components/CameraImportPrompt.vue";

const appStore = useAppStore();
const galleryStore = useGalleryStore();
const libraryStore = useLibraryStore();
useKeyboardNav();

const showCameraGallery = computed(
  () => galleryStore.camera !== null && galleryStore.images.length > 0
);
const showImport = computed(() => galleryStore.importState !== "idle");
const isLibraryMode = computed(() => appStore.appMode === "library");
const isCameraMode = computed(() => appStore.appMode === "camera");
const hasDestination = computed(() => !!appStore.destinationPath);

// Show startup screen while config loads OR while library is initially loading
const isStartingUp = computed(
  () => appStore.isInitializing || (isLibraryMode.value && libraryStore.isLoading && libraryStore.images.length === 0)
);

let unlistenMount: UnlistenFn;
let unlistenUnmount: UnlistenFn;

onMounted(async () => {
  // Load persisted config (destination path)
  await appStore.loadPersistedConfig();

  // If we have a persisted destination, load the library
  if (appStore.destinationPath) {
    await libraryStore.loadLibrary(appStore.destinationPath);
  }

  // Sync import destination from persisted config
  if (appStore.destinationPath && !galleryStore.importDestination) {
    galleryStore.importDestination = appStore.destinationPath;
  }

  // Scan for already-connected cameras. Not awaited — the library should
  // render immediately; if a camera is found, a non-blocking prompt appears
  // over the library asking the user whether to import.
  void galleryStore.scanCamera();

  // Listen for camera mount/unmount events
  unlistenMount = await listen<CameraVolume>("camera-mounted", (event) => {
    // setCameraFromEvent no longer triggers a catalog — it just records the
    // camera and requests the import prompt. The user consents via the modal
    // (or the "Camera" button in the library header) before we touch the card.
    galleryStore.setCameraFromEvent(event.payload);
  });

  unlistenUnmount = await listen<{ name: string }>("camera-unmounted", () => {
    galleryStore.clearCameraState();
    // Dismiss any stale import prompt for the now-disconnected device.
    appStore.dismissImportPrompt();
    // If in camera mode when camera disconnects, go back to library
    if (appStore.appMode === "camera") {
      appStore.switchToLibrary();
    }
  });
});

onUnmounted(() => {
  unlistenMount?.();
  unlistenUnmount?.();
});
</script>

<template>
  <div class="app">
    <!-- Startup loading screen -->
    <Transition name="startup-fade">
      <div v-if="isStartingUp" class="startup-screen" key="startup">
        <div class="startup-content">
          <div class="startup-aperture">
            <div class="startup-ring startup-ring-outer"></div>
            <div class="startup-ring startup-ring-inner"></div>
            <svg
              class="startup-iris"
              width="48"
              height="48"
              viewBox="0 0 48 48"
              fill="none"
            >
              <path
                v-for="i in 6"
                :key="i"
                class="startup-blade"
                :style="{ animationDelay: `${(i - 1) * 0.5}s` }"
                :transform="`rotate(${(i - 1) * 60} 24 24)`"
                d="M24 10 L27.5 17 L24 19 L20.5 17 Z"
                stroke="currentColor"
                stroke-width="1"
                fill="currentColor"
              />
            </svg>
          </div>
          <h1 class="startup-title">FUJI CULLER</h1>
          <p class="startup-status">Loading library...</p>
        </div>
      </div>
    </Transition>

    <!-- Library Mode -->
    <template v-if="!isStartingUp && isLibraryMode">
      <template v-if="hasDestination">
        <LibraryHeader />
        <main class="main-content">
          <LibraryGrid v-if="libraryStore.viewMode === 'grid'" />
          <LibraryViewer v-else />
        </main>
      </template>
      <EmptyState v-else />
    </template>

    <!-- Camera Mode -->
    <template v-else-if="!isStartingUp && isCameraMode">
      <template v-if="showCameraGallery">
        <AppHeader />
        <main class="main-content">
          <ImageGrid v-if="galleryStore.viewMode === 'grid'" />
          <ImageCompare v-else-if="galleryStore.viewMode === 'compare'" />
          <ImageViewer v-else />
        </main>
      </template>
      <EmptyState v-else />
    </template>

    <ImportProgress v-if="showImport" />

    <!-- Camera detection prompt. Renders over any mode; guards its own
         visibility via appStore.shouldShowImportPrompt. -->
    <CameraImportPrompt />
  </div>
</template>

<style scoped>
.app {
  height: 100vh;
  display: flex;
  flex-direction: column;
  background: var(--color-bg);
}

.main-content {
  flex: 1;
  overflow: hidden;
}

/* Startup loading screen */
.startup-screen {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: radial-gradient(ellipse at 50% 40%, #1a1816 0%, var(--color-bg) 70%);
  z-index: 50;
}

.startup-content {
  text-align: center;
  animation: slideUp 0.5s var(--ease-out) both;
}

.startup-aperture {
  position: relative;
  width: 100px;
  height: 100px;
  margin: 0 auto 28px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.startup-ring {
  position: absolute;
  border-radius: 50%;
  border: 1px solid;
}

.startup-ring-outer {
  width: 100px;
  height: 100px;
  border-color: var(--color-border);
  animation: pulse-ring 3s ease-in-out infinite;
}

.startup-ring-inner {
  width: 74px;
  height: 74px;
  border-color: var(--color-border-subtle);
  animation: pulse-ring 3s ease-in-out infinite 0.5s;
}

.startup-iris {
  color: var(--color-accent);
  animation: startup-breathe 3s ease-in-out infinite;
}

.startup-blade {
  fill-opacity: 0.1;
  stroke-opacity: 0.2;
  animation: startup-blade-glow 3s ease-in-out infinite;
}

.startup-title {
  font-family: var(--font-display);
  font-size: 28px;
  font-weight: 800;
  color: var(--color-text);
  letter-spacing: 0.08em;
  margin-bottom: 10px;
}

.startup-status {
  font-size: 13px;
  color: var(--color-text-muted);
  letter-spacing: 0.02em;
  animation: fadeIn 0.6s ease 0.3s both;
}

/* Startup exit transition */
.startup-fade-leave-active {
  transition: all 0.4s var(--ease-out);
}

.startup-fade-leave-to {
  opacity: 0;
  transform: scale(0.98);
}

@keyframes startup-breathe {
  0%, 100% {
    transform: rotate(0deg) scale(1);
  }
  50% {
    transform: rotate(30deg) scale(0.92);
  }
}

@keyframes startup-blade-glow {
  0%, 100% {
    fill-opacity: 0.1;
    stroke-opacity: 0.2;
  }
  16.67% {
    fill-opacity: 0.3;
    stroke-opacity: 0.7;
  }
}
</style>
