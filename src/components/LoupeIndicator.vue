<script setup lang="ts">
/**
 * Zoom level readout for the single-image viewers' loupe. Floats over the
 * bottom of the photo while zoomed; says so when the zoom is waiting for the
 * full-resolution image instead of showing a scaled-up placeholder.
 */
defineProps<{
  /** Zoom in percent (100 = one image pixel per screen pixel); null at fit,
   *  or while a wheel/pinch zoom waits for the image to know its level. */
  percent: number | null;
  /** Zoom requested but the full-resolution image hasn't loaded yet. */
  loading: boolean;
}>();
</script>

<template>
  <Transition name="loupe-indicator">
    <div
      v-if="percent !== null || loading"
      class="loupe-indicator"
      role="status"
      aria-live="polite"
    >
      <span v-if="percent !== null" class="loupe-percent">{{ percent }}%</span>
      <template v-if="loading">
        <span v-if="percent !== null" class="loupe-sep">&middot;</span>
        <span class="loupe-note">loading full resolution</span>
      </template>
    </div>
  </Transition>
</template>

<style scoped>
.loupe-indicator {
  position: absolute;
  left: 50%;
  bottom: 14px;
  transform: translateX(-50%);
  z-index: 11;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 100px;
  background: rgba(13, 12, 10, 0.72);
  backdrop-filter: blur(8px);
  border: 1px solid rgba(196, 162, 78, 0.25);
  pointer-events: none;
  white-space: nowrap;
}

.loupe-percent {
  font-size: 11px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: var(--color-accent);
}

.loupe-sep {
  font-size: 10px;
  color: var(--color-border-hover);
}

.loupe-note {
  font-size: 11px;
  color: var(--color-text-secondary);
}

.loupe-indicator-enter-active,
.loupe-indicator-leave-active {
  transition: opacity var(--transition-medium);
}

.loupe-indicator-enter-from,
.loupe-indicator-leave-to {
  opacity: 0;
}
</style>
