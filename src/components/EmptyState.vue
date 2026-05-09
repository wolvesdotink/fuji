<script setup lang="ts">
import { computed, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "@/stores/app";
import { useGalleryStore } from "@/stores/gallery";
import { useLibraryStore } from "@/stores/library";
import { cameraDiagnostics, type CameraDiagnostics } from "@/lib/commands";

const appStore = useAppStore();
const galleryStore = useGalleryStore();
const libraryStore = useLibraryStore();

const isLibraryMode = computed(() => appStore.appMode === "library");
const isCameraMode = computed(() => appStore.appMode === "camera");
// Show the back-to-library escape hatch only when there's actually a
// library to return to. Without a destination the user would land on
// the library's own "choose folder" empty state, which is confusing
// mid-catalog.
const canReturnToLibrary = computed(
  () => isCameraMode.value && !!appStore.destinationPath
);

function backToLibrary() {
  appStore.cancelImportAndReturnToLibrary();
}

// --- Diagnostics panel state ---
const diagnostics = ref<CameraDiagnostics | null>(null);
const isDiagnosing = ref(false);
const diagnoseError = ref<string | null>(null);

async function runDiagnostics() {
  isDiagnosing.value = true;
  diagnoseError.value = null;
  try {
    diagnostics.value = await cameraDiagnostics();
  } catch (e) {
    diagnoseError.value = String(e);
  } finally {
    isDiagnosing.value = false;
  }
}

async function copyDiagnostics() {
  if (!diagnostics.value) return;
  try {
    await navigator.clipboard.writeText(JSON.stringify(diagnostics.value, null, 2));
  } catch (e) {
    console.error("Clipboard write failed:", e);
  }
}

const detectionMessage = computed(() => {
  const err = galleryStore.detectionError;
  if (!err) return null;
  if (err === "no-camera") {
    return "No camera detected. Make sure it's powered on, connected via USB, and set to a PTP mode (e.g. \"USB RAW CONV / Backup Restore\").";
  }
  return `Scan failed: ${err}`;
});

// True when a camera is connected and being cataloged. PTP catalog takes
// ~45s for a full card, so we show a clear loading state to avoid the
// "why does it say connect your camera when my camera is plugged in?" effect.
const isCatalogInProgress = computed(
  () => galleryStore.camera !== null && galleryStore.isCataloging
);

const cameraName = computed(() => galleryStore.camera?.name ?? "camera");

async function pickFolder() {
  const dir = await open({
    directory: true,
    title: "Choose library folder",
    defaultPath: "/Volumes/LaCie",
  });

  if (dir) {
    await appStore.setDestination(dir as string);
    await libraryStore.loadLibrary(dir as string);
  }
}
</script>

<template>
  <div class="empty-state">
    <div class="empty-content">
      <!-- Viewfinder-inspired icon -->
      <div class="viewfinder">
        <div class="ring ring-outer"></div>
        <div class="ring ring-inner"></div>
        <div class="viewfinder-icon">
          <!-- Library mode: folder icon, Camera mode: camera icon -->
          <svg
            v-if="isLibraryMode"
            width="44"
            height="44"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
          <svg
            v-else
            width="44"
            height="44"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z" />
            <circle cx="12" cy="13" r="4" />
          </svg>
        </div>
      </div>

      <h1 class="title">FUJI CULLER</h1>

      <template v-if="isLibraryMode">
        <p class="subtitle">Choose a folder to view your photo library</p>
        <p class="hint">Select the folder where your imported photos are stored</p>

        <button class="action-button" @click="pickFolder">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
          Choose Folder
        </button>
      </template>

      <template v-else-if="isCatalogInProgress">
        <p class="subtitle">Cataloging {{ cameraName }}&hellip;</p>
        <p class="hint">This can take up to a minute for a full card. The gallery will appear automatically when ready.</p>

        <div class="catalog-progress">
          <svg
            class="scan-icon spinning"
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
          >
            <path d="M21 12a9 9 0 1 1-6.219-8.56" />
          </svg>
          <span>Reading images from camera</span>
        </div>

        <div v-if="canReturnToLibrary" class="back-to-library-row">
          <button class="back-to-library" @click="backToLibrary">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polyline points="15 18 9 12 15 6" />
            </svg>
            Back to Library
          </button>
        </div>
      </template>

      <template v-else>
        <p class="subtitle">Connect your Fuji camera to begin</p>
        <p class="hint">Your camera will be detected automatically when mounted</p>

        <p v-if="detectionMessage" class="detection-error">{{ detectionMessage }}</p>

        <div class="button-row">
          <button
            class="action-button"
            :disabled="galleryStore.isScanning"
            @click="galleryStore.scanCamera()"
          >
            <svg
              :class="['scan-icon', { spinning: galleryStore.isScanning }]"
              width="15"
              height="15"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
            >
              <path d="M21 12a9 9 0 1 1-6.219-8.56" />
            </svg>
            {{ galleryStore.isScanning ? "Scanning\u2026" : "Scan for Camera" }}
          </button>

          <button
            class="action-button action-button-secondary"
            :disabled="isDiagnosing"
            @click="runDiagnostics"
          >
            {{ isDiagnosing ? "Running\u2026" : "Run Diagnostics" }}
          </button>

          <button
            v-if="canReturnToLibrary"
            class="action-button action-button-secondary"
            @click="backToLibrary"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polyline points="15 18 9 12 15 6" />
            </svg>
            Back to Library
          </button>
        </div>

        <div v-if="diagnostics || diagnoseError" class="diagnostics-panel">
          <div v-if="diagnoseError" class="diag-error">{{ diagnoseError }}</div>
          <template v-if="diagnostics">
            <div class="diag-row">
              <span class="diag-label">Sidecar:</span>
              <span :class="['diag-value', diagnostics.ptp.binary_exists ? 'ok' : 'bad']">
                {{ diagnostics.ptp.binary_exists ? "found" : "missing" }}
              </span>
              <span class="diag-path">{{ diagnostics.ptp.binary_path }}</span>
            </div>
            <div v-if="diagnostics.ptp.invocation_error" class="diag-row">
              <span class="diag-label">Invocation:</span>
              <span class="diag-value bad">{{ diagnostics.ptp.invocation_error }}</span>
            </div>
            <div class="diag-row">
              <span class="diag-label">Scan stdout:</span>
              <code class="diag-code">{{ diagnostics.ptp.scan_stdout.trim() || "(empty)" }}</code>
            </div>
            <div v-if="diagnostics.ptp.scan_stderr" class="diag-row">
              <span class="diag-label">Scan stderr:</span>
              <code class="diag-code">{{ diagnostics.ptp.scan_stderr.trim() }}</code>
            </div>
            <div class="diag-row">
              <span class="diag-label">Codesign:</span>
              <code class="diag-code">{{ diagnostics.codesign.trim() }}</code>
            </div>
            <div class="diag-row">
              <span class="diag-label">Volumes:</span>
              <code class="diag-code">
                {{
                  diagnostics.volumes.length === 0
                    ? "(none)"
                    : diagnostics.volumes.map(v => `${v.name}${v.has_dcim ? " [DCIM]" : ""}`).join(", ")
                }}
              </code>
            </div>
            <button class="diag-copy" @click="copyDiagnostics">Copy JSON</button>
          </template>
        </div>
      </template>

      <div class="shortcuts">
        <span class="shortcut"><kbd>1</kbd>&ndash;<kbd>5</kbd> Rate</span>
        <span class="shortcut-sep">&middot;</span>
        <span class="shortcut"><kbd>0</kbd> Skip</span>
        <span class="shortcut-sep">&middot;</span>
        <span class="shortcut"><kbd>Space</kbd> Next unrated</span>
        <span class="shortcut-sep">&middot;</span>
        <span class="shortcut"><kbd>G</kbd> Grid</span>
        <span class="shortcut-sep">&middot;</span>
        <span class="shortcut"><kbd>/</kbd> Search</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.empty-state {
  height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  background: radial-gradient(ellipse at 50% 40%, #1a1816 0%, var(--color-bg) 70%);
}

.empty-content {
  text-align: center;
  animation: slideUp 0.5s var(--ease-out) both;
}

.viewfinder {
  position: relative;
  width: 120px;
  height: 120px;
  margin: 0 auto 32px;
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
  width: 120px;
  height: 120px;
  border-color: var(--color-border);
  animation: pulse-ring 3s ease-in-out infinite;
}

.ring-inner {
  width: 90px;
  height: 90px;
  border-color: var(--color-border-subtle);
  animation: pulse-ring 3s ease-in-out infinite 0.5s;
}

.viewfinder-icon {
  color: var(--color-text-muted);
  z-index: 1;
}

.title {
  font-family: var(--font-display);
  font-size: 32px;
  font-weight: 800;
  color: var(--color-text);
  margin-bottom: 10px;
  letter-spacing: 0.08em;
}

.subtitle {
  font-size: 16px;
  font-weight: 400;
  color: var(--color-text-secondary);
  margin-bottom: 6px;
}

.hint {
  font-size: 13px;
  color: var(--color-text-muted);
  margin-bottom: 36px;
}

.action-button {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  background: var(--color-surface);
  color: var(--color-text);
  border: 1px solid var(--color-border);
  padding: 11px 28px;
  border-radius: var(--radius-md);
  font-family: var(--font-body);
  font-size: 14px;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-medium);
}

.action-button:hover:not(:disabled) {
  background: var(--color-surface-hover);
  border-color: var(--color-border-hover);
  transform: translateY(-1px);
}

.action-button:active:not(:disabled) {
  transform: translateY(0);
}

.action-button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.action-button-secondary {
  background: transparent;
  font-weight: 400;
}

.button-row {
  display: inline-flex;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: center;
}

.detection-error {
  max-width: 520px;
  margin: 0 auto 20px;
  padding: 10px 16px;
  background: rgba(255, 120, 90, 0.08);
  border: 1px solid rgba(255, 120, 90, 0.3);
  border-radius: var(--radius-md);
  font-size: 13px;
  color: var(--color-text-secondary);
  line-height: 1.5;
}

.diagnostics-panel {
  margin: 24px auto 0;
  max-width: 640px;
  padding: 16px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  font-family: var(--font-body);
  font-size: 12px;
  text-align: left;
  animation: fadeIn 0.3s ease;
}

.diag-row {
  display: flex;
  gap: 8px;
  margin-bottom: 8px;
  flex-wrap: wrap;
  align-items: baseline;
}

.diag-label {
  font-weight: 600;
  color: var(--color-text-secondary);
  min-width: 90px;
  flex-shrink: 0;
}

.diag-value {
  font-weight: 500;
}

.diag-value.ok {
  color: #7dd67d;
}

.diag-value.bad {
  color: #ff7a5a;
}

.diag-path {
  color: var(--color-text-muted);
  font-family: var(--font-mono, monospace);
  font-size: 11px;
  word-break: break-all;
}

.diag-code {
  font-family: var(--font-mono, monospace);
  font-size: 11px;
  color: var(--color-text);
  background: rgba(0, 0, 0, 0.25);
  padding: 4px 8px;
  border-radius: 4px;
  white-space: pre-wrap;
  word-break: break-word;
  flex: 1;
  min-width: 0;
}

.diag-error {
  color: #ff7a5a;
  margin-bottom: 12px;
}

.diag-copy {
  margin-top: 12px;
  padding: 6px 12px;
  background: transparent;
  border: 1px solid var(--color-border);
  border-radius: 4px;
  color: var(--color-text-secondary);
  font-size: 11px;
  cursor: pointer;
}

.diag-copy:hover {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.scan-icon {
  flex-shrink: 0;
}

.scan-icon.spinning {
  animation: spin 1s linear infinite;
}

.catalog-progress {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  padding: 11px 22px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  color: var(--color-text-secondary);
  font-size: 13px;
  font-weight: 500;
}

.back-to-library-row {
  margin-top: 18px;
  display: flex;
  justify-content: center;
}

.back-to-library {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: transparent;
  color: var(--color-text-muted);
  border: 1px solid var(--color-border-subtle);
  padding: 7px 14px;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.back-to-library:hover {
  border-color: var(--color-border-hover);
  color: var(--color-text-secondary);
  background: var(--color-surface-hover);
}

.shortcuts {
  margin-top: 48px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  animation: fadeIn 0.6s ease 0.3s both;
}

.shortcut {
  font-size: 12px;
  color: var(--color-text-muted);
  display: flex;
  align-items: center;
  gap: 5px;
}

.shortcut-sep {
  color: var(--color-border);
  font-size: 10px;
}

.shortcut kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 20px;
  height: 20px;
  padding: 0 5px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 4px;
  font-family: var(--font-body);
  font-size: 10px;
  font-weight: 600;
  color: var(--color-text-secondary);
}
</style>
