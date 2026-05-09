import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type {
  CameraVolume,
  ImagePair,
  SelectionChoice,
  ImportSelection,
  ImportProgress,
} from "@/types";
import { homeDir, join } from "@tauri-apps/api/path";
import {
  scanForCameras,
  listImages,
  ptpListImages,
  ptpDownloadFile,
  ptpImportFiles,
  ptpDeleteFiles,
  generateThumbnails,
  importFiles,
  getFilesToDelete,
  deleteFromCamera,
  readFileRatings,
} from "@/lib/commands";
import { useAppStore } from "@/stores/app";

export const useGalleryStore = defineStore("gallery", () => {
  // Camera state
  const camera = ref<CameraVolume | null>(null);
  const isScanning = ref(false);
  // Last scan outcome for the empty-state UI. "no-camera" means scan succeeded
  // but returned zero results; any other value is a hard error message.
  const detectionError = ref<string | null>(null);

  // Images
  const images = ref<ImagePair[]>([]);
  const thumbnailPaths = ref<Map<string, string>>(new Map());
  const isLoadingThumbnails = ref(false);
  const thumbnailProgress = ref({ completed: 0, total: 0 });
  // True while `ptpListImages` is running. PTP catalog takes ~45s for a full
  // card — without this flag the UI shows "Connect your Fuji camera" during the
  // wait, which looks broken.
  const isCataloging = ref(false);
  // User-facing error surfaced when loadImages() fails. Null when OK or still
  // loading. Reset on next successful load or when the user retries.
  const loadError = ref<string | null>(null);
  // Raw error string for debug/diagnostics panels. May differ from loadError
  // when we translate known errors into a friendlier message.
  const loadErrorRaw = ref<string | null>(null);

  // Star ratings (replaces selections)
  const ratings = ref<Map<string, number>>(new Map());
  const currentIndex = ref(0);

  // Compare mode: IDs the user has flagged (via M) to review side-by-side.
  // Insertion order is preserved by Set semantics so panes stay stable.
  const markedForCompare = ref<Set<string>>(new Set());

  // Import
  const importState = ref<"idle" | "preparing" | "importing" | "complete" | "error">("idle");
  const importProgress = ref<ImportProgress | null>(null);
  const importDestination = ref<string>("");
  const importError = ref<string>("");
  // ms timestamp when the current import was kicked off — used for elapsed/ETA
  const importStartedAt = ref<number | null>(null);

  // View
  const viewMode = ref<"grid" | "single" | "compare">("single");

  // Computed
  const currentImage = computed(() => images.value[currentIndex.value] ?? null);

  const unreviewed = computed(() =>
    images.value.filter((img) => !ratings.value.has(img.id))
  );

  // Derive selection from rating
  function selectionFromRating(rating: number): SelectionChoice {
    if (rating === 0) return "Skip";
    if (rating <= 3) return "HeifOnly";
    return "HeifAndRaw";
  }

  const selectedForImport = computed(() =>
    images.value.filter((img) => {
      const rating = ratings.value.get(img.id);
      return rating && rating > 0;
    })
  );

  const selectionSummary = computed(() => {
    let skip = 0;
    let heifOnly = 0;
    let heifAndRaw = 0;

    for (const img of images.value) {
      const rating = ratings.value.get(img.id);
      if (!rating || rating === 0) {
        skip++;
      } else if (rating <= 3) {
        heifOnly++;
      } else {
        heifAndRaw++;
      }
    }

    return {
      total: images.value.length,
      skip,
      heifOnly,
      heifAndRaw,
      reviewed: heifOnly + heifAndRaw + (images.value.length - unreviewed.value.length - heifOnly - heifAndRaw),
      remaining: unreviewed.value.length,
      toImport: heifOnly + heifAndRaw,
    };
  });

  const totalImportSize = computed(() => {
    let bytes = 0;
    for (const img of images.value) {
      const rating = ratings.value.get(img.id);
      if (!rating || rating === 0) continue;
      bytes += img.hif_size;
      if (rating >= 4 && img.raf_size) {
        bytes += img.raf_size;
      }
    }
    return bytes;
  });

  const canImport = computed(
    () =>
      importDestination.value &&
      selectedForImport.value.length > 0 &&
      importState.value === "idle"
  );

  // Helpers
  function isPtp() {
    return camera.value?.source_type === "Ptp";
  }

  /** Extract the bare filename from a ptp:// path */
  function ptpFileName(ptpPath: string): string {
    const idx = ptpPath.lastIndexOf("/");
    return idx >= 0 ? ptpPath.substring(idx + 1) : ptpPath;
  }

  // PTP preview cache: maps image id → local file path
  const ptpPreviewCache = ref<Map<string, string>>(new Map());

  // Actions
  async function scanCamera() {
    isScanning.value = true;
    detectionError.value = null;
    try {
      const cameras = await scanForCameras();
      if (cameras.length > 0) {
        const found = cameras[0];
        // If the PTP poller or camera-mounted event already set the same
        // camera, don't re-prompt — requestImportPrompt dedups but we also
        // avoid clobbering existing camera/images state here.
        if (camera.value && camera.value.mount_path === found.mount_path) {
          detectionError.value = null;
        } else {
          camera.value = found;
          detectionError.value = null;
          // Don't switch mode or auto-load — ask the user first via the
          // import prompt. Library stays on-screen; loadImages() runs only
          // when the user confirms (via modal or the Camera button in the
          // library header).
          useAppStore().requestImportPrompt(found.mount_path);
        }
      } else {
        detectionError.value = "no-camera";
      }
    } catch (e) {
      console.error("Failed to scan for cameras:", e);
      detectionError.value = String(e);
    } finally {
      isScanning.value = false;
    }
  }

  /**
   * Translate raw backend errors into user-facing copy.
   * PTP catalog/download errors commonly surface as "Camera not found: NAME"
   * or "ptp-bridge daemon exited" — neither is meaningful to the user. Map
   * those to a reconnect hint so the UI can prompt the obvious fix.
   */
  function humanizeLoadError(raw: string): string {
    const msg = raw.trim();
    if (
      /^Camera (not found|disconnected)/i.test(msg) ||
      /ptp-bridge daemon exited/i.test(msg) ||
      /ptp-bridge request timed out/i.test(msg) ||
      /Failed to open session/i.test(msg)
    ) {
      return "Camera disconnected. Reconnect the camera via USB and try again.";
    }
    return msg;
  }

  /**
   * Retry the last loadImages() attempt after a "camera disconnected" error.
   * Re-runs scanCamera() first so the newly-reconnected camera is picked up.
   */
  async function retryLoadImages() {
    loadError.value = null;
    loadErrorRaw.value = null;
    await scanCamera();
    if (camera.value) {
      await loadImages();
    }
  }

  async function loadImages() {
    if (!camera.value) return;
    loadError.value = null;
    loadErrorRaw.value = null;
    try {
      if (isPtp()) {
        // PTP camera: use catalog command which also generates thumbnails
        const home = await homeDir();
        const cacheDir = await join(home, ".cache", "fuji-culler", "ptp-thumbs");
        isCataloging.value = true;
        try {
          images.value = await ptpListImages(camera.value.mount_path, cacheDir);
        } finally {
          isCataloging.value = false;
        }

        // Populate thumbnail paths from the known cache naming convention
        // The Swift bridge saves thumbnails as <stem>_thumb.jpg
        for (const img of images.value) {
          const thumbPath = await join(cacheDir, `${img.id}_thumb.jpg`);
          thumbnailPaths.value.set(img.id, thumbPath);
        }
        // Trigger reactivity
        thumbnailPaths.value = new Map(thumbnailPaths.value);
      } else {
        // Mass storage: use filesystem listing (with persistent index)
        const home = await homeDir();
        const indexCacheDir = await join(home, ".cache", "fuji-culler");
        images.value = await listImages(camera.value.dcim_path, indexCacheDir);
      }
      ratings.value = new Map();
      currentIndex.value = 0;

      // Read camera-set ratings from HIF files (mass storage only)
      if (!isPtp()) {
        try {
          const hifPaths = images.value.map((img) => img.hif_path);
          const fileRatings = await readFileRatings(hifPaths);
          for (const [stem, rating] of Object.entries(fileRatings)) {
            ratings.value.set(stem, rating);
          }
        } catch (e) {
          console.error("Failed to read camera ratings:", e);
        }
      }

      // For mass-storage cameras, generate thumbnails from RAF files
      if (!isPtp()) {
        await loadThumbnails();
      }
      // For PTP cameras, thumbnails were already generated during catalog
    } catch (e) {
      const raw = String(e);
      console.error("Failed to load images:", raw);
      loadErrorRaw.value = raw;
      loadError.value = humanizeLoadError(raw);
    }
  }

  async function loadThumbnails() {
    if (!camera.value || images.value.length === 0) return;

    const imagesWithRaf = images.value.filter((img) => img.raf_path);
    if (imagesWithRaf.length === 0) return;

    isLoadingThumbnails.value = true;
    thumbnailProgress.value = { completed: 0, total: imagesWithRaf.length };

    const home = await homeDir();
    const cacheDir = await join(home, ".cache", "fuji-culler", "thumbs");

    try {
      await generateThumbnails(
        camera.value.dcim_path,
        imagesWithRaf.map((img) => img.id),
        imagesWithRaf.map((img) => img.raf_path!),
        cacheDir,
        (progress) => {
          thumbnailPaths.value.set(progress.image_id, progress.thumbnail_path);
          thumbnailProgress.value = {
            completed: progress.completed,
            total: progress.total,
          };
        }
      );
    } catch (e) {
      console.error("Failed to generate thumbnails:", e);
    } finally {
      isLoadingThumbnails.value = false;
    }
  }

  function setRating(imageId: string, rating: number) {
    // Mutate the Map directly — Vue 3 wraps Map collections reactively, so
    // per-key observers invalidate, but we avoid triggering every computed
    // that touches `ratings.value` (which a full Map reassignment would do).
    if (rating === 0) {
      ratings.value.delete(imageId);
    } else {
      ratings.value.set(imageId, rating);
    }
  }

  function navigateNext() {
    if (currentIndex.value < images.value.length - 1) {
      currentIndex.value++;
    }
  }

  function navigatePrev() {
    if (currentIndex.value > 0) {
      currentIndex.value--;
    }
  }

  function jumpToNextUnreviewed() {
    const startIdx = currentIndex.value + 1;
    for (let i = startIdx; i < images.value.length; i++) {
      if (!ratings.value.has(images.value[i].id)) {
        currentIndex.value = i;
        return;
      }
    }
    // Wrap around
    for (let i = 0; i < startIdx && i < images.value.length; i++) {
      if (!ratings.value.has(images.value[i].id)) {
        currentIndex.value = i;
        return;
      }
    }
  }

  function rateAndAdvance(rating: number) {
    const img = currentImage.value;
    if (img) {
      setRating(img.id, rating);
      jumpToNextUnreviewed();
    }
  }

  /**
   * Toggle whether an image is flagged for side-by-side comparison.
   * We reassign the ref with a new Set so computeds that read the whole
   * collection (e.g. pane lists in ImageCompare) invalidate reliably —
   * Vue's Set reactivity tracks per-key access, but pane arrays iterate
   * the whole collection.
   */
  function toggleMarkForCompare(imageId: string) {
    const next = new Set(markedForCompare.value);
    if (next.has(imageId)) {
      next.delete(imageId);
    } else {
      next.add(imageId);
    }
    markedForCompare.value = next;
  }

  function clearMarkedForCompare() {
    markedForCompare.value = new Set();
  }

  /** Enter compare view if the user has flagged at least 2 images. */
  function openCompareView() {
    if (markedForCompare.value.size >= 2) {
      viewMode.value = "compare";
    }
  }

  async function startImport() {
    if (!canImport.value) return;

    // Flip state synchronously BEFORE doing any work, so the loading screen
    // paints on the next frame — no gap between click and visible feedback.
    importState.value = "preparing";
    importStartedAt.value = Date.now();
    importError.value = "";
    importProgress.value = null;

    const importSelections: ImportSelection[] = images.value
      .filter((img) => {
        const rating = ratings.value.get(img.id);
        return rating && rating > 0;
      })
      .map((img) => {
        const rating = ratings.value.get(img.id)!;
        return {
          image_id: img.id,
          choice: selectionFromRating(rating),
          hif_path: img.hif_path,
          raf_path: img.raf_path,
          rating,
        };
      });

    const onProgress = (progress: ImportProgress) => {
      // First progress event flips us out of "preparing" into live "importing"
      if (importState.value === "preparing") importState.value = "importing";
      importProgress.value = progress;
    };

    try {
      if (isPtp() && camera.value) {
        // PTP import: download from camera via ptp-bridge
        await ptpImportFiles(
          camera.value.mount_path,
          importSelections,
          importDestination.value,
          onProgress
        );
      } else {
        // Mass storage import: direct file copy
        await importFiles(importSelections, importDestination.value, onProgress);
      }
      importState.value = "complete";
    } catch (e) {
      importState.value = "error";
      importError.value = String(e);
      console.error("Import failed:", e);
    }
  }

  async function clearCamera() {
    const importSelections: ImportSelection[] = images.value
      .filter((img) => {
        const rating = ratings.value.get(img.id);
        return rating && rating > 0;
      })
      .map((img) => {
        const rating = ratings.value.get(img.id)!;
        return {
          image_id: img.id,
          choice: selectionFromRating(rating),
          hif_path: img.hif_path,
          raf_path: img.raf_path,
          rating,
        };
      });

    try {
      if (isPtp() && camera.value) {
        // PTP delete: extract file names from ptp:// paths
        const fileNames: string[] = [];
        for (const sel of importSelections) {
          if (sel.choice === "Skip") continue;
          fileNames.push(ptpFileName(sel.hif_path));
          if (sel.raf_path) {
            fileNames.push(ptpFileName(sel.raf_path));
          }
        }
        const deletedCount = await ptpDeleteFiles(camera.value.mount_path, fileNames);
        return deletedCount;
      } else {
        // Mass storage delete: use filesystem paths
        const filesToDelete = await getFilesToDelete(importSelections);
        const deletedCount = await deleteFromCamera(filesToDelete);
        return deletedCount;
      }
    } catch (e) {
      console.error("Failed to delete from camera:", e);
      throw e;
    }
  }

  async function clearAllFromCamera() {
    if (images.value.length === 0) return 0;

    try {
      if (isPtp() && camera.value) {
        // PTP delete: extract file names from ptp:// paths
        const fileNames: string[] = [];
        for (const img of images.value) {
          fileNames.push(ptpFileName(img.hif_path));
          if (img.raf_path) {
            fileNames.push(ptpFileName(img.raf_path));
          }
        }
        const deletedCount = await ptpDeleteFiles(camera.value.mount_path, fileNames);
        return deletedCount;
      } else {
        // Mass storage delete: collect all file paths
        const filePaths: string[] = [];
        for (const img of images.value) {
          filePaths.push(img.hif_path);
          if (img.raf_path) {
            filePaths.push(img.raf_path);
          }
        }
        const deletedCount = await deleteFromCamera(filePaths);
        return deletedCount;
      }
    } catch (e) {
      console.error("Failed to delete all files from camera:", e);
      throw e;
    }
  }

  /**
   * For PTP images, download the HIF to a local cache so it can be displayed.
   * Returns the local file path. Caches results so each file is downloaded once.
   */
  async function ensurePtpPreview(imageId: string, hifPath: string): Promise<string> {
    // Check cache first
    const cached = ptpPreviewCache.value.get(imageId);
    if (cached) return cached;

    if (!camera.value || !hifPath.startsWith("ptp://")) {
      return hifPath; // Not a PTP path, return as-is
    }

    const home = await homeDir();
    const cacheDir = await join(home, ".cache", "fuji-culler", "ptp-preview");
    const fileName = ptpFileName(hifPath);

    const localPath = await ptpDownloadFile(camera.value.mount_path, fileName, cacheDir);
    ptpPreviewCache.value = new Map(ptpPreviewCache.value.set(imageId, localPath));
    return localPath;
  }

  function setCameraFromEvent(vol: CameraVolume) {
    const appStore = useAppStore();
    // If we already have this exact camera set and a catalog is running or
    // images are loaded, skip — the PTP poller and the initial `scanCamera`
    // can both fire for the same device on startup.
    if (
      camera.value &&
      camera.value.mount_path === vol.mount_path &&
      (isCataloging.value || images.value.length > 0)
    ) {
      return;
    }
    // Also skip if scanCamera already populated state for this same device
    // and an import prompt is pending — both sources race for the first few
    // seconds; whichever wins sets the prompt, the other no-ops.
    if (
      camera.value &&
      camera.value.mount_path === vol.mount_path &&
      appStore.showImportPrompt &&
      appStore.pendingCameraMountPath === vol.mount_path
    ) {
      return;
    }
    camera.value = vol;
    // Ask the user before switching mode or reading the card. The prompt
    // (requestImportPrompt) is idempotent so it's safe even if scanCamera
    // already fired for this device.
    appStore.requestImportPrompt(vol.mount_path);
  }

  function clearCameraState() {
    camera.value = null;
    images.value = [];
    ratings.value = new Map();
    markedForCompare.value = new Set();
    currentIndex.value = 0;
    importState.value = "idle";
    importProgress.value = null;
    importStartedAt.value = null;
  }

  return {
    // State
    camera,
    isScanning,
    detectionError,
    images,
    thumbnailPaths,
    isLoadingThumbnails,
    thumbnailProgress,
    isCataloging,
    loadError,
    loadErrorRaw,
    ratings,
    markedForCompare,
    currentIndex,
    importState,
    importProgress,
    importStartedAt,
    importDestination,
    importError,
    viewMode,

    // Computed
    currentImage,
    unreviewed,
    selectedForImport,
    selectionSummary,
    totalImportSize,
    canImport,

    // PTP state
    ptpPreviewCache,

    // Actions
    isPtp,
    ensurePtpPreview,
    scanCamera,
    loadImages,
    retryLoadImages,
    setRating,
    rateAndAdvance,
    toggleMarkForCompare,
    clearMarkedForCompare,
    openCompareView,
    selectionFromRating,
    navigateNext,
    navigatePrev,
    jumpToNextUnreviewed,
    startImport,
    clearCamera,
    clearAllFromCamera,
    setCameraFromEvent,
    clearCameraState,
  };
});
