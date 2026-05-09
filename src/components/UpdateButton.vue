<script setup lang="ts">
/**
 * UpdateButton — header update affordance.
 *
 * Hidden by default. Surfaces only when the updater state machine in
 * `useUpdater` reports something the user can act on:
 *
 *   available   →  ↑ icon button with accent dot. Click installs.
 *   downloading →  spinner + percentage pill. Not clickable.
 *   ready       →  ⟲ accent button labelled "RESTART". Click relaunches.
 *
 * `idle`, `checking`, `error`, and the dismissed-`available` case all render
 * nothing — the button only surfaces when there's a real update the user can
 * act on. Failed checks stay silent; the next boot-time check will retry.
 *
 * Visual language matches the surrounding LibraryHeader buttons so the
 * affordance feels native rather than imported from another design system.
 */
import { computed } from "vue";
import { useUpdater } from "@/composables/useUpdater";

const updater = useUpdater();

const progressPct = computed(() => {
  if (updater.totalBytes.value <= 0) return null;
  return Math.min(
    99,
    Math.floor((updater.downloaded.value / updater.totalBytes.value) * 100)
  );
});

const availableTitle = computed(() => {
  const v = updater.newVersion.value;
  return v
    ? `Update available (${v}) — click to install`
    : "Update available — click to install";
});

const availableLabel = computed(() => {
  const v = updater.newVersion.value;
  return v ? `Install update ${v}` : "Install update";
});
</script>

<template>
  <button
    v-if="updater.status.value === 'available' && !updater.dismissed.value"
    type="button"
    class="update-btn"
    :title="availableTitle"
    :aria-label="availableLabel"
    @click="updater.install()"
  >
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <line x1="12" y1="19" x2="12" y2="5" />
      <polyline points="5 12 12 5 19 12" />
    </svg>
    <span class="update-dot" aria-hidden="true"></span>
  </button>

  <div
    v-else-if="updater.status.value === 'downloading'"
    class="update-progress"
    title="Installing update"
    aria-live="polite"
  >
    <span class="progress-spinner" aria-hidden="true"></span>
    <span>{{ progressPct === null ? "DL" : `${progressPct}%` }}</span>
  </div>

  <button
    v-else-if="updater.status.value === 'ready'"
    type="button"
    class="update-restart"
    title="Restart to apply the update"
    aria-label="Restart to apply the update"
    @click="updater.restart()"
  >
    <svg
      width="13"
      height="13"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <polyline points="23 4 23 10 17 10" />
      <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
    </svg>
    <span>RESTART</span>
  </button>
</template>

<style scoped>
.update-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  background: var(--color-bg);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  cursor: pointer;
  position: relative;
  transition: all var(--transition-fast);
}

.update-btn:hover {
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.update-dot {
  position: absolute;
  top: 5px;
  right: 5px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-accent);
  box-shadow: 0 0 4px rgba(196, 162, 78, 0.5);
}

.update-progress {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 10px;
  background: var(--color-bg);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  font-family: var(--font-body);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.04em;
  user-select: none;
}

.progress-spinner {
  width: 10px;
  height: 10px;
  border: 1.5px solid var(--color-border);
  border-top-color: var(--color-accent);
  border-radius: 50%;
  animation: update-spin 0.8s linear infinite;
}

.update-restart {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 12px;
  background: var(--color-accent);
  color: #0d0c0a;
  border: none;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.04em;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.update-restart:hover {
  background: var(--color-accent-hover);
  transform: translateY(-1px);
  box-shadow: 0 4px 12px rgba(196, 162, 78, 0.2);
}

.update-restart:active {
  transform: translateY(0);
}

@keyframes update-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
