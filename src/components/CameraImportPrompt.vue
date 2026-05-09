<script setup lang="ts">
import { computed, onMounted, onUnmounted } from "vue";
import { useAppStore } from "@/stores/app";
import { useGalleryStore } from "@/stores/gallery";

const appStore = useAppStore();
const galleryStore = useGalleryStore();

// Additional guards beyond `shouldShowImportPrompt`: hide the prompt if the
// camera state has diverged from the pending mount path (e.g. device was
// unplugged but the unmount handler hasn't fired yet), or if a catalog is
// already running / images already loaded for this device.
const visible = computed(() => {
  if (!appStore.shouldShowImportPrompt) return false;
  const cam = galleryStore.camera;
  if (!cam) return false;
  if (cam.mount_path !== appStore.pendingCameraMountPath) return false;
  if (galleryStore.isCataloging) return false;
  if (galleryStore.images.length > 0) return false;
  return true;
});

const cameraName = computed(() => galleryStore.camera?.name ?? "camera");

function onImport() {
  appStore.confirmImportFromCamera();
}

function onDismiss() {
  appStore.dismissImportPrompt();
}

function onKeydown(e: KeyboardEvent) {
  if (!visible.value) return;
  if (e.key === "Escape") {
    e.preventDefault();
    onDismiss();
  } else if (e.key === "Enter") {
    e.preventDefault();
    onImport();
  }
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
});
</script>

<template>
  <Transition name="prompt-fade">
    <div v-if="visible" class="prompt-overlay" @click.self="onDismiss">
      <div class="prompt-modal" role="dialog" aria-modal="true" aria-labelledby="prompt-title">
        <div class="prompt-icon">
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z" />
            <circle cx="12" cy="13" r="4" />
          </svg>
        </div>

        <h2 id="prompt-title" class="prompt-title">Camera detected</h2>
        <p class="prompt-camera-name">{{ cameraName }}</p>
        <p class="prompt-body">Do you want to import images from this camera?</p>

        <div class="prompt-actions">
          <button class="prompt-button prompt-button-secondary" @click="onDismiss">
            Not now
          </button>
          <button class="prompt-button prompt-button-primary" @click="onImport" autofocus>
            Import
          </button>
        </div>

        <p class="prompt-hint">
          <kbd>Enter</kbd> to import &middot; <kbd>Esc</kbd> to dismiss
        </p>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.prompt-overlay {
  position: fixed;
  inset: 0;
  background: rgba(5, 5, 4, 0.7);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 90;
  backdrop-filter: blur(10px);
}

.prompt-modal {
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-lg);
  padding: 32px 36px;
  min-width: 380px;
  max-width: 440px;
  text-align: center;
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
  animation: promptSlideUp 0.3s var(--ease-out);
}

.prompt-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 64px;
  height: 64px;
  margin: 0 auto 18px;
  border-radius: 50%;
  background: var(--color-heif-bg);
  color: var(--color-heif);
}

.prompt-title {
  font-family: var(--font-display);
  font-size: 22px;
  font-weight: 700;
  color: var(--color-text);
  margin: 0 0 4px;
  letter-spacing: 0.04em;
}

.prompt-camera-name {
  font-size: 14px;
  color: var(--color-accent);
  font-weight: 600;
  margin: 0 0 14px;
  letter-spacing: 0.02em;
}

.prompt-body {
  font-size: 14px;
  color: var(--color-text-secondary);
  line-height: 1.5;
  margin: 0 0 24px;
}

.prompt-actions {
  display: flex;
  gap: 10px;
  justify-content: center;
  margin-bottom: 16px;
}

.prompt-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 10px 22px;
  border-radius: var(--radius-md);
  font-family: var(--font-body);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-medium);
  min-width: 110px;
}

.prompt-button-primary {
  background: var(--color-accent);
  color: #0d0c0a;
  border: none;
}

.prompt-button-primary:hover {
  background: var(--color-accent-hover);
  transform: translateY(-1px);
  box-shadow: 0 4px 12px rgba(196, 162, 78, 0.2);
}

.prompt-button-primary:active {
  transform: translateY(0);
}

.prompt-button-secondary {
  background: transparent;
  color: var(--color-text-secondary);
  border: 1px solid var(--color-border);
}

.prompt-button-secondary:hover {
  background: var(--color-surface-hover);
  border-color: var(--color-border-hover);
  color: var(--color-text);
}

.prompt-hint {
  font-size: 11px;
  color: var(--color-text-muted);
  margin: 0;
  letter-spacing: 0.02em;
}

.prompt-hint kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  background: var(--color-bg);
  border: 1px solid var(--color-border);
  border-radius: 3px;
  font-family: var(--font-body);
  font-size: 10px;
  font-weight: 600;
  color: var(--color-text-secondary);
  margin: 0 2px;
}

/* Enter/leave transitions */
.prompt-fade-enter-active,
.prompt-fade-leave-active {
  transition: opacity 0.2s ease;
}

.prompt-fade-enter-from,
.prompt-fade-leave-to {
  opacity: 0;
}

@keyframes promptSlideUp {
  from {
    opacity: 0;
    transform: translateY(8px) scale(0.98);
  }
  to {
    opacity: 1;
    transform: translateY(0) scale(1);
  }
}
</style>
