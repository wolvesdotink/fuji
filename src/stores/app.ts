import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { homeDir, join } from "@tauri-apps/api/path";
import { loadConfig, saveConfig } from "@/lib/commands";
// Note: `useGalleryStore` is referenced inside function bodies only (never at
// module-load time). A static import here would create a bidirectional
// module-eval cycle with stores/gallery.ts; since Pinia's `defineStore`
// returns a lazily-invoked composable, the dynamic `await import()` inside
// confirmImportFromCamera() keeps the cycle purely at call time.

export const useAppStore = defineStore("app", () => {
  const appMode = ref<"library" | "camera">("library");
  const destinationPath = ref<string>("");
  const configPath = ref<string>("");
  const isInitializing = ref(true);

  // Import prompt state — shown when a camera is detected but the user hasn't
  // yet consented to enter camera mode. Lives here (not in gallery store)
  // because this is a mode/UI decision, not gallery data.
  const showImportPrompt = ref(false);
  // Identifies which camera triggered the prompt. If the device disappears
  // the prompt self-dismisses via the `shouldShowImportPrompt` guard.
  const pendingCameraMountPath = ref<string | null>(null);

  async function getConfigPath(): Promise<string> {
    if (configPath.value) return configPath.value;
    const home = await homeDir();
    configPath.value = await join(home, ".cache", "fuji-culler", "config.json");
    return configPath.value;
  }

  async function loadPersistedConfig() {
    isInitializing.value = true;
    try {
      const path = await getConfigPath();
      const config = await loadConfig(path);
      if (config.destination_path) {
        destinationPath.value = config.destination_path;
      }
    } catch (e) {
      console.error("Failed to load config:", e);
    } finally {
      isInitializing.value = false;
    }
  }

  async function setDestination(path: string) {
    destinationPath.value = path;
    try {
      const cfgPath = await getConfigPath();
      await saveConfig(cfgPath, { destination_path: path });
    } catch (e) {
      console.error("Failed to save config:", e);
    }
  }

  function switchToLibrary() {
    appMode.value = "library";
  }

  function switchToCamera() {
    appMode.value = "camera";
  }

  /**
   * Called by galleryStore when a camera is detected (startup scan or live
   * mount event). Queues the confirmation prompt. Idempotent — no-op if the
   * user is already in camera mode, or if the same camera is already pending.
   */
  function requestImportPrompt(mountPath: string) {
    if (appMode.value === "camera") return;
    if (showImportPrompt.value && pendingCameraMountPath.value === mountPath) {
      return;
    }
    pendingCameraMountPath.value = mountPath;
    showImportPrompt.value = true;
  }

  /**
   * User confirmed: switch into camera mode and start the catalog.
   * Uses a lazy import of the gallery store to avoid a Pinia init cycle.
   */
  async function confirmImportFromCamera() {
    showImportPrompt.value = false;
    pendingCameraMountPath.value = null;
    const { useGalleryStore } = await import("@/stores/gallery");
    const galleryStore = useGalleryStore();
    if (!galleryStore.camera) return;
    switchToCamera();
    // Fire-and-forget — `isCataloging` drives the loading UI.
    if (galleryStore.images.length === 0) {
      void galleryStore.loadImages();
    }
  }

  /** User declined the prompt. Camera state is preserved so the library
   *  header's "Camera" button can still trigger import later. */
  function dismissImportPrompt() {
    showImportPrompt.value = false;
    pendingCameraMountPath.value = null;
  }

  /** Back-to-library from camera mode (gallery or empty state). Does not
   *  clear camera state or abort an in-flight catalog. */
  function cancelImportAndReturnToLibrary() {
    switchToLibrary();
  }

  // Computed guard for the prompt visibility — also defends against stale
  // prompts when the device has since disappeared or the user is already
  // in camera mode.
  const shouldShowImportPrompt = computed(() => {
    if (!showImportPrompt.value) return false;
    if (appMode.value !== "library") return false;
    return pendingCameraMountPath.value !== null;
  });

  return {
    appMode,
    destinationPath,
    isInitializing,
    showImportPrompt,
    pendingCameraMountPath,
    shouldShowImportPrompt,
    loadPersistedConfig,
    setDestination,
    switchToLibrary,
    switchToCamera,
    requestImportPrompt,
    confirmImportFromCamera,
    dismissImportPrompt,
    cancelImportAndReturnToLibrary,
  };
});
