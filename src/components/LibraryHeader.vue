<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "@/stores/app";
import { useLibraryStore } from "@/stores/library";
import { useGalleryStore } from "@/stores/gallery";
import UpdateButton from "@/components/UpdateButton.vue";

const appStore = useAppStore();
const libraryStore = useLibraryStore();
const galleryStore = useGalleryStore();

const searchInput = ref<HTMLInputElement | null>(null);
const localQuery = ref("");
let debounceTimer: ReturnType<typeof setTimeout> | null = null;

const folderName = computed(() => {
  if (!appStore.destinationPath) return "";
  return appStore.destinationPath.split("/").filter(Boolean).slice(-2).join("/");
});

const cameraConnected = computed(() => galleryStore.camera !== null);

const imageCount = computed(() => {
  if (libraryStore.searchResults) {
    return `${libraryStore.displayImages.length} of ${libraryStore.images.length}`;
  }
  return `${libraryStore.images.length}`;
});

const indexStatusText = computed(() => {
  if (libraryStore.isDownloadingModel) {
    const p = libraryStore.modelDownloadProgress;
    if (p.bytes_total > 0) {
      const mb = (p.bytes_downloaded / 1024 / 1024).toFixed(0);
      const totalMb = (p.bytes_total / 1024 / 1024).toFixed(0);
      return `Downloading AI model... ${mb}/${totalMb} MB`;
    }
    return "Downloading AI model...";
  }
  if (libraryStore.isIndexing) {
    return `Indexing ${libraryStore.indexProgress.completed}/${libraryStore.indexProgress.total}`;
  }
  return null;
});

watch(localQuery, (val) => {
  if (debounceTimer) clearTimeout(debounceTimer);
  if (!val.trim()) {
    libraryStore.clearSearch();
    return;
  }
  debounceTimer = setTimeout(() => {
    libraryStore.searchImages(val);
  }, 300);
});

async function pickDestination() {
  const dir = await open({
    directory: true,
    title: "Choose library folder",
    defaultPath: appStore.destinationPath || "/Volumes/LaCie",
  });

  if (dir) {
    await appStore.setDestination(dir as string);
    await libraryStore.loadLibrary(dir as string);
  }
}

function focusSearch() {
  searchInput.value?.focus();
}

function clearSearch() {
  localQuery.value = "";
  libraryStore.clearSearch();
}

function openCamera() {
  // Clicking the header's Camera button IS the consent — go straight into
  // camera mode and start the catalog if images aren't yet loaded. Skip
  // the modal prompt (the button visibility already confirms a camera is
  // connected and the user has deliberately clicked it).
  appStore.confirmImportFromCamera();
}

defineExpose({ focusSearch });
</script>

<template>
  <header class="library-header">
    <div class="header-main">
      <div class="header-left">
        <div class="library-info">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="folder-icon">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
          <button class="folder-name" @click="pickDestination" :title="appStore.destinationPath">
            {{ folderName || 'Choose folder' }}
          </button>
        </div>
        <div class="header-meta">
          <span class="image-count">{{ imageCount }} media</span>
          <template v-if="indexStatusText">
            <span class="meta-sep">&middot;</span>
            <span class="index-status">
              <span class="index-spinner"></span>
              {{ indexStatusText }}
            </span>
          </template>
        </div>
      </div>

      <div class="header-center">
        <div class="search-bar">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="search-icon">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            ref="searchInput"
            v-model="localQuery"
            type="text"
            class="search-input"
            placeholder="Search photos and videos with AI..."
            :disabled="!libraryStore.isIndexReady && !libraryStore.isIndexing"
          />
          <div class="search-status" v-if="libraryStore.isSearching">
            <span class="search-spinner"></span>
          </div>
          <button
            v-else-if="localQuery"
            class="search-clear"
            @click="clearSearch"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
          <kbd class="search-hint" v-if="!localQuery">/</kbd>
        </div>
      </div>

      <div class="header-right">
        <UpdateButton />

        <div class="sort-toggle">
          <button
            :class="['toggle-btn', { active: libraryStore.sortBy === 'created' }]"
            @click="libraryStore.sortBy = 'created'"
            title="Sort by created date"
          >Created</button>
          <button
            :class="['toggle-btn', { active: libraryStore.sortBy === 'updated' }]"
            @click="libraryStore.sortBy = 'updated'"
            title="Sort by updated date"
          >Updated</button>
          <button
            :class="['toggle-btn', { active: libraryStore.sortBy === 'stars' }]"
            @click="libraryStore.sortBy = 'stars'"
            title="Sort by star rating"
          >Stars</button>
        </div>

        <div class="view-toggle">
          <button
            :class="['toggle-btn', { active: libraryStore.viewMode === 'single' }]"
            @click="libraryStore.viewMode = 'single'"
            title="Single view"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="3" />
            </svg>
          </button>
          <button
            :class="['toggle-btn', { active: libraryStore.viewMode === 'grid' }]"
            @click="libraryStore.viewMode = 'grid'"
            title="Grid view"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="7" height="7" rx="1.5" />
              <rect x="14" y="3" width="7" height="7" rx="1.5" />
              <rect x="3" y="14" width="7" height="7" rx="1.5" />
              <rect x="14" y="14" width="7" height="7" rx="1.5" />
            </svg>
          </button>
        </div>

        <button
          class="camera-button"
          @click="openCamera"
          :disabled="!cameraConnected"
          :title="cameraConnected ? 'Open Camera' : 'No camera connected'"
        >
          <span class="camera-dot" v-if="cameraConnected"></span>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z" />
            <circle cx="12" cy="13" r="4" />
          </svg>
          <span>{{ cameraConnected ? galleryStore.camera?.name : 'Camera' }}</span>
        </button>
      </div>
    </div>
  </header>
</template>

<style scoped>
.library-header {
  flex-shrink: 0;
  background: var(--color-surface);
  border-bottom: 1px solid var(--color-border);
}

.header-main {
  display: grid;
  grid-template-columns: 1fr minmax(200px, 400px) 1fr;
  align-items: center;
  padding: 10px 20px;
  gap: 16px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 14px;
  min-width: 0;
}

.library-info {
  display: flex;
  align-items: center;
  gap: 8px;
}

.folder-icon {
  color: var(--color-accent);
  flex-shrink: 0;
}

.folder-name {
  background: none;
  border: none;
  font-weight: 600;
  font-size: 13px;
  color: var(--color-text);
  white-space: nowrap;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  transition: all var(--transition-fast);
  font-family: var(--font-body);
}

.folder-name:hover {
  background: var(--color-surface-hover);
  color: var(--color-accent);
}

.header-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--color-text-muted);
}

.meta-sep {
  color: var(--color-border);
}

.image-count {
  color: var(--color-text-secondary);
  font-variant-numeric: tabular-nums;
}

.index-status {
  display: flex;
  align-items: center;
  gap: 5px;
  color: var(--color-accent);
  font-size: 11px;
}

.index-spinner {
  width: 10px;
  height: 10px;
  border: 1.5px solid var(--color-border);
  border-top-color: var(--color-accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

/* Search bar */
.header-center {
}

.search-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  background: var(--color-bg);
  border: 1px solid var(--color-border-subtle);
  border-radius: var(--radius-sm);
  padding: 6px 12px;
  transition: all var(--transition-fast);
}

.search-bar:focus-within {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 2px var(--color-accent-dim);
}

.search-icon {
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.search-input {
  flex: 1;
  background: none;
  border: none;
  color: var(--color-text);
  font-family: var(--font-body);
  font-size: 13px;
  outline: none;
  min-width: 0;
}

.search-input::placeholder {
  color: var(--color-text-muted);
}

.search-input:disabled {
  opacity: 0.5;
}

.search-clear {
  background: none;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 2px;
  display: flex;
  align-items: center;
  border-radius: 3px;
  transition: all var(--transition-fast);
}

.search-clear:hover {
  color: var(--color-text);
  background: var(--color-surface-hover);
}

.search-hint {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 18px;
  padding: 0 4px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 3px;
  font-family: var(--font-body);
  font-size: 10px;
  font-weight: 600;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.search-spinner {
  width: 12px;
  height: 12px;
  border: 1.5px solid var(--color-border);
  border-top-color: var(--color-accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

/* Right section */
.header-right {
  display: flex;
  align-items: center;
  gap: 8px;
  justify-self: end;
}

.sort-toggle,
.view-toggle {
  display: flex;
  background: var(--color-bg);
  border-radius: var(--radius-sm);
  padding: 2px;
  border: 1px solid var(--color-border-subtle);
}

.sort-toggle .toggle-btn {
  font-size: 11px;
  font-family: var(--font-body);
  font-weight: 500;
  padding: 5px 10px;
}

.toggle-btn {
  background: none;
  border: none;
  color: var(--color-text-muted);
  padding: 5px 7px;
  border-radius: 4px;
  cursor: pointer;
  display: flex;
  align-items: center;
  transition: all var(--transition-fast);
}

.toggle-btn.active {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.toggle-btn:hover:not(.active) {
  color: var(--color-text-secondary);
}

.camera-button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--color-bg);
  color: var(--color-text-muted);
  border: 1px solid var(--color-border);
  padding: 6px 14px;
  border-radius: var(--radius-sm);
  font-family: var(--font-body);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  white-space: nowrap;
  transition: all var(--transition-fast);
}

.camera-button:hover:not(:disabled) {
  border-color: var(--color-heif);
  color: var(--color-heif-light);
  background: var(--color-heif-bg);
}

.camera-button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.camera-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-heif);
  box-shadow: 0 0 6px rgba(61, 148, 101, 0.4);
  flex-shrink: 0;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
</style>
