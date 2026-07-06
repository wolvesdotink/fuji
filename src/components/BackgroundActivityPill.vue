<script setup lang="ts">
import { computed, watch } from "vue";
import { useAppStore } from "@/stores/app";
import { useGalleryStore } from "@/stores/gallery";

/**
 * Floating pill surfacing work that continues after its screen was dismissed:
 * an import running (or finished) with the overlay hidden, or a camera
 * catalog running (or done) while the user is back in the library. Clicking
 * it returns to the relevant screen. Renders in every app mode.
 */
const appStore = useAppStore();
const galleryStore = useGalleryStore();

const importHidden = computed(
  () => galleryStore.importState !== "idle" && !galleryStore.importScreenVisible
);
const importRunning = computed(
  () =>
    importHidden.value &&
    (galleryStore.importState === "preparing" || galleryStore.importState === "importing")
);
const importComplete = computed(
  () => importHidden.value && galleryStore.importState === "complete"
);
const importFailed = computed(
  () => importHidden.value && galleryStore.importState === "error"
);

// Camera pills only make sense while the user is in the library — in camera
// mode the catalog progress / gallery is already on screen.
const cataloging = computed(
  () => appStore.appMode === "library" && galleryStore.isCataloging && !importHidden.value
);
const cameraReady = computed(
  () =>
    appStore.appMode === "library" &&
    !importHidden.value &&
    !galleryStore.isCataloging &&
    galleryStore.cameraLoadedNotice &&
    galleryStore.images.length > 0
);

// Entering camera mode consumes the "photos ready" notice regardless of how
// the user got there (pill click, header camera button, prompt).
watch(
  () => appStore.appMode,
  (mode) => {
    if (mode === "camera") galleryStore.cameraLoadedNotice = false;
  }
);

const visible = computed(
  () =>
    importRunning.value ||
    importComplete.value ||
    importFailed.value ||
    cataloging.value ||
    cameraReady.value
);

const importLabel = computed(() => {
  const p = galleryStore.importProgress;
  if (!p) return "Preparing import…";
  if (p.phase === "ImportingToPhotos") return "Importing to Apple Photos…";
  if (p.phase === "Verifying") return "Verifying copies…";
  if (p.bytes_total > 0) {
    const pct = Math.round((p.bytes_copied / p.bytes_total) * 100);
    return `Importing… ${pct}%`;
  }
  if (p.files_total > 0) {
    return `Importing… ${p.files_completed} / ${p.files_total} files`;
  }
  return "Importing…";
});

const readyCount = computed(() => galleryStore.images.length);

function handleClick() {
  if (importHidden.value) {
    galleryStore.importScreenVisible = true;
  } else {
    galleryStore.cameraLoadedNotice = false;
    appStore.switchToCamera();
  }
}

function dismissReady(e: MouseEvent) {
  e.stopPropagation();
  galleryStore.cameraLoadedNotice = false;
}
</script>

<template>
  <Transition name="pill">
    <button
      v-if="visible"
      class="activity-pill"
      :class="{ 'is-complete': importComplete || cameraReady, 'is-error': importFailed }"
      @click="handleClick"
    >
      <!-- Spinner for in-flight work -->
      <span v-if="importRunning || cataloging" class="pill-spinner"></span>

      <!-- Check for finished work -->
      <svg
        v-else-if="importComplete || cameraReady"
        width="13"
        height="13"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <polyline points="20 6 9 17 4 12" />
      </svg>

      <!-- X for failure -->
      <svg
        v-else
        width="13"
        height="13"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2.5"
        stroke-linecap="round"
      >
        <line x1="18" y1="6" x2="6" y2="18" />
        <line x1="6" y1="6" x2="18" y2="18" />
      </svg>

      <span v-if="importRunning" class="pill-label">{{ importLabel }}</span>
      <span v-else-if="importComplete" class="pill-label">Import complete — click to review</span>
      <span v-else-if="importFailed" class="pill-label">Import failed — click for details</span>
      <span v-else-if="cataloging" class="pill-label">Reading camera…</span>
      <span v-else class="pill-label">
        {{ readyCount }} {{ readyCount === 1 ? "photo" : "photos" }} ready on camera
      </span>

      <span
        v-if="cameraReady"
        class="pill-dismiss"
        title="Dismiss"
        @click="dismissReady"
      >
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </span>
    </button>
  </Transition>
</template>

<style scoped>
.activity-pill {
  position: fixed;
  bottom: 22px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 90; /* below the import screen (100), above everything else */
  display: inline-flex;
  align-items: center;
  gap: 9px;
  padding: 9px 18px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 999px;
  color: var(--color-text-secondary);
  font-family: var(--font-body);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.45);
  transition: border-color var(--transition-fast), color var(--transition-fast);
}

.activity-pill:hover {
  border-color: var(--color-border-hover);
  color: var(--color-text);
}

.activity-pill.is-complete {
  border-color: rgba(148, 196, 142, 0.4);
  color: var(--color-heif);
}

.activity-pill.is-error {
  border-color: rgba(194, 66, 66, 0.4);
  color: var(--color-skip-light);
}

.pill-spinner {
  width: 12px;
  height: 12px;
  border: 1.5px solid var(--color-border);
  border-top-color: var(--color-accent);
  border-radius: 50%;
  animation: pill-spin 0.8s linear infinite;
  flex-shrink: 0;
}

.pill-label {
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.pill-dismiss {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  margin: -2px -6px -2px 0;
  border-radius: 50%;
  color: var(--color-text-muted);
  transition: all var(--transition-fast);
}

.pill-dismiss:hover {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.pill-enter-active,
.pill-leave-active {
  transition: opacity 0.25s ease, transform 0.25s var(--ease-out);
}

.pill-enter-from,
.pill-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(8px);
}

@keyframes pill-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
