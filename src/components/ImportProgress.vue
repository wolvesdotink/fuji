<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useGalleryStore } from "@/stores/gallery";
import { useAppStore } from "@/stores/app";
import { useLibraryStore } from "@/stores/library";

const store = useGalleryStore();
const appStore = useAppStore();
const libraryStore = useLibraryStore();
const isDeleting = ref(false);
const deleteResult = ref<string | null>(null);
const showConfirmDelete = ref(false);

const isDeletingAll = ref(false);
const showConfirmDeleteAll = ref(false);

const progress = computed(() => store.importProgress);
const isPreparing = computed(() => store.importState === "preparing");
const isImporting = computed(() => store.importState === "importing");
const isComplete = computed(() => store.importState === "complete");
const isError = computed(() => store.importState === "error");
const skippedCount = computed(() => store.selectionSummary.skip);

// --- Live clock for elapsed / ETA (ticks every 500ms while the screen is up)
const now = ref(Date.now());
let tickHandle: number | undefined;
onMounted(() => {
  tickHandle = window.setInterval(() => (now.value = Date.now()), 500);
  window.addEventListener("keydown", handleEscape, true);
});
onUnmounted(() => {
  if (tickHandle !== undefined) clearInterval(tickHandle);
  window.removeEventListener("keydown", handleEscape, true);
});

// The elapsed counter freezes on the store's terminal timestamp (not a local
// one) so the value survives hiding and re-opening this screen mid/after import.
const elapsedMs = computed(() => {
  const start = store.importStartedAt;
  if (!start) return 0;
  return (store.importFinishedAt ?? now.value) - start;
});

const etaMs = computed<number | null>(() => {
  const p = progress.value;
  if (!p) return null;
  if (p.phase !== "CopyingToLaCie") return null;
  if (p.bytes_total === 0 || p.bytes_copied === 0) return null;
  const rate = p.bytes_copied / Math.max(elapsedMs.value, 1); // bytes/ms
  if (rate <= 0) return null;
  const remaining = p.bytes_total - p.bytes_copied;
  return remaining / rate;
});

const percentBytes = computed(() => {
  if (!progress.value || progress.value.bytes_total === 0) return 0;
  return Math.round(
    (progress.value.bytes_copied / progress.value.bytes_total) * 100
  );
});

const phaseTitle = computed(() => {
  if (isPreparing.value) return "Preparing import";
  if (!progress.value) return "Starting up";
  switch (progress.value.phase) {
    case "CopyingToLaCie":
      return store.camera ? "Downloading from camera" : "Copying files";
    case "ImportingToPhotos":
      return "Importing to Apple Photos";
    case "Verifying":
      return "Verifying copies";
    case "Complete":
      return "Import complete";
    default:
      return "";
  }
});

// Phase index for the stepper dots (1-based)
const phaseIndex = computed(() => {
  if (!progress.value) return 1;
  switch (progress.value.phase) {
    case "CopyingToLaCie":
      return 1;
    case "ImportingToPhotos":
      return 2;
    case "Verifying":
      return 3;
    case "Complete":
      return 3;
    default:
      return 1;
  }
});

// Indeterminate bar for phases where we don't have meaningful bytes progress
const isIndeterminate = computed(() => {
  if (isPreparing.value) return true;
  if (!progress.value) return true;
  if (progress.value.phase === "ImportingToPhotos") return true;
  if (progress.value.phase === "Verifying") return true;
  // PTP download reports bytes_total=0
  if (
    progress.value.phase === "CopyingToLaCie" &&
    progress.value.bytes_total === 0
  )
    return true;
  return false;
});

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = m.toString().padStart(h > 0 ? 2 : 1, "0");
  const ss = sec.toString().padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

async function confirmAndDelete() {
  showConfirmDelete.value = true;
}

async function executeDelete() {
  isDeleting.value = true;
  showConfirmDelete.value = false;
  try {
    const count = await store.clearCamera();
    deleteResult.value = `Successfully deleted ${count} files from camera.`;
  } catch (e) {
    deleteResult.value = `Error: ${e}`;
  } finally {
    isDeleting.value = false;
  }
}

async function executeDeleteAll() {
  isDeletingAll.value = true;
  showConfirmDeleteAll.value = false;
  try {
    const count = await store.clearAllFromCamera();
    deleteResult.value = `Successfully deleted ${count} files from camera.`;
  } catch (e) {
    deleteResult.value = `Error: ${e}`;
  } finally {
    isDeletingAll.value = false;
  }
}

const allDone = computed(() => !!deleteResult.value);

function dismiss() {
  store.importState = "idle";
  store.importProgress = null;
  store.importStartedAt = null;
  deleteResult.value = null;
  showConfirmDelete.value = false;
  showConfirmDeleteAll.value = false;
  // Navigate back to library and refresh with newly imported images
  appStore.switchToLibrary();
  if (store.importDestination) {
    libraryStore.loadLibrary(store.importDestination);
  }
}

const inFlight = computed(() => isPreparing.value || isImporting.value);

/**
 * Hide the overlay without touching the import — the copy runs on a Rust
 * worker thread and keeps going. BackgroundActivityPill offers the way back.
 */
function hideScreen() {
  store.importScreenVisible = false;
}

// Escape hides the screen while an import is in flight. Registered in the
// capture phase with stopPropagation so useKeyboardNav's window listener
// doesn't also act on the same keypress underneath the overlay.
function handleEscape(e: KeyboardEvent) {
  if (e.key !== "Escape" || !inFlight.value) return;
  e.preventDefault();
  e.stopPropagation();
  hideScreen();
}
</script>

<template>
  <div
    class="import-screen"
    :class="{
      'state-progress': inFlight,
      'state-complete': isComplete,
      'state-error': isError,
    }"
  >
    <button
      v-if="inFlight"
      class="hide-button"
      @click="hideScreen"
      title="Hide — import continues in the background (Esc)"
    >
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="6 9 12 15 18 9" />
      </svg>
      <span>Hide</span>
    </button>
    <div class="import-content">
      <!-- Aperture / iris -->
      <div class="aperture" :class="{ 'is-spinning': inFlight, 'is-success': isComplete, 'is-error': isError }">
        <div class="ring ring-outer"></div>
        <div class="ring ring-inner"></div>
        <svg
          v-if="!isComplete && !isError"
          class="iris"
          width="64"
          height="64"
          viewBox="0 0 48 48"
          fill="none"
        >
          <path
            v-for="i in 6"
            :key="i"
            class="iris-blade"
            :style="{ animationDelay: `${(i - 1) * 0.5}s` }"
            :transform="`rotate(${(i - 1) * 60} 24 24)`"
            d="M24 10 L27.5 17 L24 19 L20.5 17 Z"
            stroke="currentColor"
            stroke-width="1"
            fill="currentColor"
          />
        </svg>
        <svg
          v-else-if="isComplete"
          class="status-check"
          width="48"
          height="48"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>
        <svg
          v-else-if="isError"
          class="status-x"
          width="48"
          height="48"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </div>

      <!-- Progress / Preparing -->
      <template v-if="inFlight">
        <h1 class="phase-title">{{ phaseTitle }}</h1>

        <p class="current-file" v-if="progress && progress.current_file">
          {{ progress.current_file }}
        </p>
        <p class="current-file subtle" v-else-if="isPreparing">
          Gathering selected files&hellip;
        </p>

        <div class="progress-track" :class="{ indeterminate: isIndeterminate }">
          <div
            class="progress-fill"
            v-if="!isIndeterminate"
            :style="{ width: `${percentBytes}%` }"
          ></div>
          <div class="progress-fill indeterminate-fill" v-else></div>
        </div>

        <!-- Stats row -->
        <div class="stats">
          <div class="stat" v-if="progress && progress.files_total > 0">
            <span class="stat-value">{{ progress.files_completed }} / {{ progress.files_total }}</span>
            <span class="stat-label">files</span>
          </div>
          <div class="stat" v-if="progress && progress.bytes_total > 0">
            <span class="stat-value">{{ formatBytes(progress.bytes_copied) }} / {{ formatBytes(progress.bytes_total) }}</span>
            <span class="stat-label">copied</span>
          </div>
          <div class="stat">
            <span class="stat-value">{{ formatDuration(elapsedMs) }}</span>
            <span class="stat-label">elapsed</span>
          </div>
          <div class="stat" v-if="etaMs !== null">
            <span class="stat-value">~{{ formatDuration(etaMs) }}</span>
            <span class="stat-label">remaining</span>
          </div>
        </div>

        <!-- Phase stepper dots -->
        <div class="phase-steps">
          <span class="step" :class="{ active: phaseIndex === 1, done: phaseIndex > 1 }">
            {{ store.camera ? 'Download' : 'Copy' }}
          </span>
          <span class="step-sep"></span>
          <span class="step" :class="{ active: phaseIndex === 2, done: phaseIndex > 2 }">
            Photos
          </span>
          <span class="step-sep"></span>
          <span class="step" :class="{ active: phaseIndex === 3 }">
            Verify
          </span>
        </div>

        <p class="background-hint">
          Press <kbd>Esc</kbd> to keep using the app — the import continues in the background
        </p>
      </template>

      <!-- Complete (pre-dismiss actions) -->
      <template v-else-if="isComplete && !allDone">
        <h1 class="phase-title">Import complete</h1>
        <p class="current-file" v-if="progress">
          {{ progress.files_total }} files &middot; {{ formatBytes(progress.bytes_total) }} &middot; {{ formatDuration(elapsedMs) }}
        </p>

        <p class="delete-result" v-if="deleteResult">{{ deleteResult }}</p>

        <!-- Confirm delete imported -->
        <template v-if="showConfirmDelete">
          <div class="confirm-section">
            <div class="confirm-warning">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="warning-icon">
                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
              </svg>
              <p>This will permanently delete the imported files from your camera's SD card. This cannot be undone.</p>
            </div>
            <div class="actions">
              <button class="btn btn-secondary" @click="showConfirmDelete = false">Cancel</button>
              <button class="btn btn-danger" @click="executeDelete">Delete from Camera</button>
            </div>
          </div>
        </template>

        <!-- Confirm delete all -->
        <template v-else-if="showConfirmDeleteAll">
          <div class="confirm-section">
            <div class="confirm-warning">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="warning-icon">
                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
              </svg>
              <p>This will permanently delete <strong>all</strong> photos from your camera's SD card &mdash; including {{ skippedCount }} skipped {{ skippedCount === 1 ? 'image' : 'images' }} that {{ skippedCount === 1 ? 'was' : 'were' }} not imported. This cannot be undone.</p>
            </div>
            <div class="actions">
              <button class="btn btn-secondary" @click="showConfirmDeleteAll = false">Cancel</button>
              <button class="btn btn-danger" @click="executeDeleteAll">Delete All</button>
            </div>
          </div>
        </template>

        <!-- Action buttons -->
        <template v-else>
          <div class="actions">
            <button class="btn btn-secondary" @click="dismiss">Done</button>
            <button
              v-if="!deleteResult"
              class="btn btn-danger-outline"
              @click="confirmAndDelete"
              :disabled="isDeleting || isDeletingAll"
            >
              {{ isDeleting ? "Deleting\u2026" : "Delete Imported Only" }}
            </button>
            <button
              v-if="!deleteResult"
              class="btn btn-danger-outline"
              @click="showConfirmDeleteAll = true"
              :disabled="isDeleting || isDeletingAll"
            >
              {{ isDeletingAll ? "Deleting\u2026" : "Delete All Photos" }}
            </button>
          </div>
        </template>
      </template>

      <!-- All done (delete completed) -->
      <template v-else-if="allDone">
        <h1 class="phase-title">All done</h1>
        <p class="current-file">{{ deleteResult }}</p>
        <div class="actions">
          <button class="btn btn-primary" @click="dismiss">Close</button>
        </div>
      </template>

      <!-- Error -->
      <template v-else-if="isError">
        <h1 class="phase-title">Import failed</h1>
        <p class="error-message">{{ store.importError }}</p>
        <div class="actions">
          <button class="btn btn-secondary" @click="dismiss">Close</button>
          <button class="btn btn-primary" @click="store.startImport()">Retry</button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.import-screen {
  position: fixed;
  inset: 0;
  background: radial-gradient(ellipse at 50% 35%, #1a1816 0%, var(--color-bg) 70%);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
  animation: screen-fade-in 0.3s var(--ease-out);
  backdrop-filter: blur(2px);
}

.import-content {
  width: min(560px, 90vw);
  text-align: center;
  animation: content-rise 0.5s var(--ease-out) both;
}

.hide-button {
  position: absolute;
  top: 18px;
  right: 20px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: transparent;
  color: var(--color-text-muted);
  border: 1px solid var(--color-border-subtle);
  padding: 6px 12px;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.hide-button:hover {
  border-color: var(--color-border-hover);
  color: var(--color-text-secondary);
  background: var(--color-surface-hover);
}

.background-hint {
  margin-top: 26px;
  font-size: 11px;
  color: var(--color-text-muted);
}

.background-hint kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 3px;
  font-family: var(--font-body);
  font-size: 10px;
  font-weight: 600;
  color: var(--color-text-secondary);
}

/* --- Aperture / iris --- */
.aperture {
  position: relative;
  width: 140px;
  height: 140px;
  margin: 0 auto 36px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.ring {
  position: absolute;
  border-radius: 50%;
  border: 1px solid;
}

.ring-outer {
  width: 140px;
  height: 140px;
  border-color: var(--color-border);
  animation: pulse-ring 3s ease-in-out infinite;
}

.ring-inner {
  width: 104px;
  height: 104px;
  border-color: var(--color-border-subtle);
  animation: pulse-ring 3s ease-in-out infinite 0.5s;
}

.iris {
  color: var(--color-accent);
  transition: color 0.4s ease;
}

.aperture.is-spinning .iris {
  animation: iris-rotate 2.4s linear infinite;
}

.iris-blade {
  fill-opacity: 0.15;
  stroke-opacity: 0.3;
  animation: blade-glow 3s ease-in-out infinite;
}

.aperture.is-success .ring-outer {
  border-color: var(--color-heif);
  animation: none;
}
.aperture.is-success .ring-inner {
  border-color: rgba(148, 196, 142, 0.35);
  animation: none;
}
.aperture.is-success .status-check {
  color: var(--color-heif);
  animation: pop-in 0.4s var(--ease-out) both;
}

.aperture.is-error .ring-outer {
  border-color: var(--color-skip);
  animation: none;
}
.aperture.is-error .ring-inner {
  border-color: rgba(194, 66, 66, 0.35);
  animation: none;
}
.aperture.is-error .status-x {
  color: var(--color-skip);
  animation: pop-in 0.4s var(--ease-out) both;
}

/* --- Title / file --- */
.phase-title {
  font-family: var(--font-display);
  font-size: 30px;
  font-weight: 800;
  letter-spacing: 0.04em;
  color: var(--color-text);
  margin: 0 0 10px;
}

.current-file {
  font-size: 14px;
  color: var(--color-text-secondary);
  margin: 0 0 32px;
  max-width: 480px;
  margin-left: auto;
  margin-right: auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.current-file.subtle {
  color: var(--color-text-muted);
  font-style: italic;
}

/* --- Progress bar --- */
.progress-track {
  position: relative;
  height: 4px;
  background: var(--color-border-subtle);
  border-radius: 2px;
  overflow: hidden;
  margin: 0 auto 24px;
  max-width: 440px;
}

.progress-fill {
  height: 100%;
  background: linear-gradient(90deg, var(--color-accent), var(--color-accent-hover));
  border-radius: 2px;
  transition: width 0.3s ease;
}

.progress-track.indeterminate .indeterminate-fill {
  width: 40%;
  animation: indeterminate-slide 1.6s ease-in-out infinite;
}

/* --- Stats --- */
.stats {
  display: flex;
  justify-content: center;
  gap: 36px;
  margin-bottom: 28px;
  flex-wrap: wrap;
}

.stat {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 3px;
  min-width: 90px;
}

.stat-value {
  font-family: var(--font-display);
  font-size: 15px;
  font-weight: 600;
  color: var(--color-text);
  font-variant-numeric: tabular-nums;
}

.stat-label {
  font-size: 11px;
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.08em;
}

/* --- Phase stepper --- */
.phase-steps {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.12em;
  color: var(--color-text-muted);
}

.step {
  padding: 4px 0;
  transition: color 0.3s ease;
}

.step.active {
  color: var(--color-accent);
}

.step.done {
  color: var(--color-text-secondary);
  text-decoration: line-through;
  text-decoration-color: var(--color-border);
}

.step-sep {
  width: 28px;
  height: 1px;
  background: var(--color-border);
}

/* --- Actions / confirms --- */
.actions {
  display: flex;
  gap: 10px;
  justify-content: center;
  margin-top: 28px;
  flex-wrap: wrap;
}

.btn {
  padding: 11px 24px;
  border-radius: var(--radius-md);
  font-family: var(--font-body);
  font-size: 14px;
  font-weight: 500;
  cursor: pointer;
  border: none;
  transition: all var(--transition-medium);
}

.btn:active {
  transform: scale(0.98);
}

.btn-primary {
  background: var(--color-accent);
  color: #0d0c0a;
}

.btn-primary:hover {
  background: var(--color-accent-hover);
  box-shadow: 0 4px 12px rgba(196, 162, 78, 0.2);
}

.btn-secondary {
  background: var(--color-bg);
  color: var(--color-text);
  border: 1px solid var(--color-border);
}

.btn-secondary:hover {
  border-color: var(--color-border-hover);
  background: var(--color-surface-hover);
}

.btn-danger {
  background: var(--color-skip);
  color: white;
}

.btn-danger:hover {
  background: #d24a4a;
}

.btn-danger-outline {
  background: none;
  color: var(--color-skip-light);
  border: 1px solid rgba(194, 66, 66, 0.3);
}

.btn-danger-outline:hover:not(:disabled) {
  background: var(--color-skip-bg);
  border-color: rgba(194, 66, 66, 0.5);
}

.btn-danger-outline:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.delete-result {
  font-size: 13px;
  color: var(--color-heif);
  margin: 0 0 8px;
}

.confirm-section {
  margin-top: 20px;
  max-width: 440px;
  margin-left: auto;
  margin-right: auto;
}

.confirm-warning {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  font-size: 13px;
  color: var(--color-skip-light);
  background: var(--color-skip-bg);
  padding: 14px 16px;
  border-radius: var(--radius-md);
  border: 1px solid rgba(194, 66, 66, 0.15);
  text-align: left;
  line-height: 1.5;
}

.confirm-warning p {
  margin: 0;
}

.warning-icon {
  flex-shrink: 0;
  margin-top: 1px;
  color: var(--color-skip);
}

.error-message {
  font-size: 13px;
  color: var(--color-skip-light);
  background: var(--color-skip-bg);
  padding: 12px 16px;
  border-radius: var(--radius-md);
  border: 1px solid rgba(194, 66, 66, 0.15);
  text-align: left;
  word-break: break-word;
  line-height: 1.5;
  margin: 0 auto 8px;
  max-width: 440px;
}

/* --- Animations --- */
@keyframes screen-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes content-rise {
  from {
    opacity: 0;
    transform: translateY(8px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

@keyframes iris-rotate {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

@keyframes blade-glow {
  0%, 100% {
    fill-opacity: 0.15;
    stroke-opacity: 0.3;
  }
  16.67% {
    fill-opacity: 0.4;
    stroke-opacity: 0.8;
  }
}

@keyframes pulse-ring {
  0%, 100% {
    opacity: 0.6;
    transform: scale(1);
  }
  50% {
    opacity: 1;
    transform: scale(1.02);
  }
}

@keyframes pop-in {
  from {
    opacity: 0;
    transform: scale(0.6);
  }
  to {
    opacity: 1;
    transform: scale(1);
  }
}

@keyframes indeterminate-slide {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(350%);
  }
}
</style>
