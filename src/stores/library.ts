import { defineStore } from "pinia";
import { ref, computed, shallowRef, watch } from "vue";
import { homeDir, join } from "@tauri-apps/api/path";
import type { LibraryImage, SearchResult } from "@/types";
import {
  listLibraryImages,
  generateLibraryThumbnails,
  ensureClipModels,
  indexLibrary,
  searchLibrary,
  readFileRatings,
  writeFileRating,
} from "@/lib/commands";

export const useLibraryStore = defineStore("library", () => {
  // Images. Image records are immutable — the array is only ever replaced
  // wholesale, never mutated in place — so shallowRef avoids deep-proxying
  // thousands of records (and re-tracking them on every dependency read).
  const images = shallowRef<LibraryImage[]>([]);
  const thumbnailPaths = ref<Map<string, string>>(new Map());
  const isLoading = ref(false);
  const isLoadingThumbnails = ref(false);
  const thumbnailProgress = ref({ completed: 0, total: 0 });

  // Navigation
  const currentIndex = ref(0);
  const viewMode = ref<"grid" | "single">("grid");
  const sortBy = ref<"created" | "updated" | "stars">("created");

  // While in single (viewer) mode we freeze the stars-sort order against a
  // snapshot of ratings taken on entry. Otherwise rating the on-screen photo
  // re-sorts displayImages beneath the viewer, and currentImage (= displayImages
  // [currentIndex]) jumps to whatever now occupies that slot. Null in grid mode
  // → live ratings drive the sort as before.
  const frozenRatings = shallowRef<Map<string, number> | null>(null);

  // Ratings
  const ratings = ref<Map<string, number>>(new Map()); // file_path → 1-5
  let writeTimeout: ReturnType<typeof setTimeout> | null = null;
  const pendingWrites = new Map<string, number>();

  // Search
  const searchQuery = ref("");
  const searchResults = ref<SearchResult[] | null>(null);
  const isSearching = ref(false);
  const isIndexing = ref(false);
  const indexProgress = ref({ completed: 0, total: 0 });
  const isIndexReady = ref(false);

  // Model download
  const isDownloadingModel = ref(false);
  const modelDownloadProgress = ref({ bytes_downloaded: 0, bytes_total: 0, file_name: "" });

  // Computed
  const displayImages = computed(() => {
    if (searchResults.value) {
      const resultIds = new Map(
        searchResults.value.map((r) => [r.image_id, r.score])
      );
      return images.value
        .filter((img) => resultIds.has(img.id))
        .sort((a, b) => {
          const scoreA = resultIds.get(a.id) ?? 0;
          const scoreB = resultIds.get(b.id) ?? 0;
          return scoreB - scoreA;
        });
    }

    const sort = sortBy.value;
    const sorted = [...images.value];

    switch (sort) {
      case "updated":
        sorted.sort((a, b) => b.date_modified - a.date_modified);
        break;
      case "stars": {
        // In single view read the frozen snapshot so rating the current photo
        // doesn't re-sort under the viewer. `?? ratings.value` short-circuits
        // in grid mode, so the sort still tracks live ratings there.
        const r = frozenRatings.value ?? ratings.value;
        sorted.sort((a, b) => {
          const rA = r.get(a.file_path) ?? 0;
          const rB = r.get(b.file_path) ?? 0;
          if (rA !== rB) return rB - rA;
          return b.date_created - a.date_created;
        });
        break;
      }
      case "created":
      default:
        sorted.sort((a, b) => b.date_created - a.date_created);
        break;
    }

    return sorted;
  });

  const currentImage = computed(
    () => displayImages.value[currentIndex.value] ?? null
  );

  // Snapshot ratings the instant we enter single view, and drop the snapshot
  // on the way back to grid. flush: "sync" is required: the snapshot must be
  // in place before displayImages re-evaluates in the same tick as the
  // viewMode flip, otherwise the first read would still see live ratings.
  watch(
    viewMode,
    (mode) => {
      frozenRatings.value = mode === "single" ? new Map(ratings.value) : null;
    },
    { flush: "sync" }
  );

  const searchScores = computed(() => {
    if (!searchResults.value) return new Map<string, number>();
    return new Map(searchResults.value.map((r) => [r.image_id, r.score]));
  });

  // Rating helpers
  function getRating(filePath: string): number {
    return ratings.value.get(filePath) ?? 0;
  }

  function setRating(filePath: string, rating: number) {
    // Mutate the Map directly — Vue 3 wraps Map collections reactively, so
    // per-key observers invalidate, but we avoid triggering every computed
    // that touches `ratings.value` (which a full Map reassignment would do).
    if (rating === 0) {
      ratings.value.delete(filePath);
    } else {
      ratings.value.set(filePath, rating);
    }

    // Debounced write to file
    pendingWrites.set(filePath, rating);
    if (writeTimeout) clearTimeout(writeTimeout);
    writeTimeout = setTimeout(() => flushPendingWrites(), 500);
  }

  async function flushPendingWrites() {
    const writes = new Map(pendingWrites);
    pendingWrites.clear();

    for (const [filePath, rating] of writes) {
      try {
        await writeFileRating(filePath, rating);
      } catch (e) {
        console.error(`Failed to write rating for ${filePath}:`, e);
      }
    }
  }

  // Actions
  async function loadLibrary(destPath: string) {
    if (!destPath) return;
    isLoading.value = true;
    try {
      const home = await homeDir();
      const indexCacheDir = await join(home, ".cache", "fuji-culler");
      images.value = await listLibraryImages(destPath, indexCacheDir);
      thumbnailPaths.value = new Map();
      currentIndex.value = 0;
      searchResults.value = null;
      searchQuery.value = "";
      ratings.value = new Map();

      if (images.value.length > 0) {
        // Read ratings from file XMP metadata concurrently. This is
        // independent of thumbnail generation, so kick it off now and apply
        // the results as soon as they resolve — the grid can render ratings
        // without waiting for thumbnails.
        const filePaths = images.value.map((img) => img.file_path);
        readFileRatings(filePaths, indexCacheDir)
          .then((fileRatings) => {
            // Build the id → image lookup once (O(n)). The previous
            // images.value.find() per returned rating was O(n) each, i.e.
            // O(n^2) across a full library of camera-set ratings.
            const imageById = new Map(images.value.map((i) => [i.id, i]));
            for (const [stem, rating] of Object.entries(fileRatings)) {
              const img = imageById.get(stem);
              if (img) {
                ratings.value.set(img.file_path, rating);
              }
            }
          })
          .catch((e) => console.error("Failed to read library ratings:", e));

        // Generate thumbnails in the background, then build the CLIP index.
        // buildSearchIndex reads thumbnailPaths, so it MUST run only after
        // thumbnails complete — hence the chained .then().
        loadThumbnails()
          .then(() => buildSearchIndex())
          .catch((e) => console.error("Background indexing failed:", e));
      }
    } catch (e) {
      console.error("Failed to load library:", e);
    } finally {
      isLoading.value = false;
    }
  }

  async function loadThumbnails() {
    if (images.value.length === 0) return;

    isLoadingThumbnails.value = true;
    thumbnailProgress.value = { completed: 0, total: images.value.length };

    const home = await homeDir();
    const cacheDir = await join(home, ".cache", "fuji-culler", "library-thumbs");

    try {
      await generateLibraryThumbnails(
        images.value.map((img) => img.file_path),
        images.value.map((img) => img.id),
        cacheDir,
        (progress) => {
          // In-place .set only — Vue wraps the Map reactively and tracks
          // has/get per key, so only the card observing this image_id
          // recomputes. Reassigning `new Map(...)` here invalidated every
          // card's thumbnail lookup on each thumbnail, an O(n) storm per
          // file → O(n^2) over a load. (Mirrors gallery.ts loadThumbnails.)
          thumbnailPaths.value.set(progress.image_id, progress.thumbnail_path);
          thumbnailProgress.value = {
            completed: progress.completed,
            total: progress.total,
          };
        }
      );
    } catch (e) {
      console.error("Failed to generate library thumbnails:", e);
    } finally {
      isLoadingThumbnails.value = false;
    }
  }

  function navigateNext() {
    if (currentIndex.value < displayImages.value.length - 1) {
      currentIndex.value++;
    }
  }

  function navigatePrev() {
    if (currentIndex.value > 0) {
      currentIndex.value--;
    }
  }

  async function buildSearchIndex() {
    if (images.value.length === 0) return;

    const home = await homeDir();
    const modelDir = await join(home, ".cache", "fuji-culler", "models");
    const indexPath = await join(home, ".cache", "fuji-culler", "clip-index.bin");

    // Ensure models are downloaded
    isDownloadingModel.value = true;
    try {
      await ensureClipModels(modelDir, (progress) => {
        modelDownloadProgress.value = progress;
      });
    } catch (e) {
      console.error("Failed to download CLIP models:", e);
      isDownloadingModel.value = false;
      return;
    }
    isDownloadingModel.value = false;

    // Build index from thumbnails
    isIndexing.value = true;
    indexProgress.value = { completed: 0, total: images.value.length };

    try {
      const thumbPaths = images.value.map(
        (img) => thumbnailPaths.value.get(img.id) ?? ""
      );

      await indexLibrary(
        images.value.map((img) => img.id),
        thumbPaths,
        modelDir,
        indexPath,
        (progress) => {
          indexProgress.value = {
            completed: progress.completed,
            total: progress.total,
          };
        }
      );
      isIndexReady.value = true;
    } catch (e) {
      console.error("Failed to build search index:", e);
    } finally {
      isIndexing.value = false;
    }
  }

  async function searchImages(query: string) {
    if (!query.trim()) {
      clearSearch();
      return;
    }

    searchQuery.value = query;
    isSearching.value = true;

    try {
      const home = await homeDir();
      const modelDir = await join(home, ".cache", "fuji-culler", "models");
      const indexPath = await join(
        home,
        ".cache",
        "fuji-culler",
        "clip-index.bin"
      );

      searchResults.value = await searchLibrary(query, modelDir, indexPath);
      currentIndex.value = 0;
    } catch (e) {
      console.error("Search failed:", e);
      searchResults.value = [];
    } finally {
      isSearching.value = false;
    }
  }

  function clearSearch() {
    searchQuery.value = "";
    searchResults.value = null;
    currentIndex.value = 0;
  }

  // Flush pending writes before page unload
  if (typeof window !== "undefined") {
    window.addEventListener("beforeunload", () => {
      if (pendingWrites.size > 0) {
        flushPendingWrites();
      }
    });
  }

  return {
    // State
    images,
    thumbnailPaths,
    isLoading,
    isLoadingThumbnails,
    thumbnailProgress,
    currentIndex,
    viewMode,
    sortBy,
    ratings,
    searchQuery,
    searchResults,
    searchScores,
    isSearching,
    isIndexing,
    indexProgress,
    isIndexReady,
    isDownloadingModel,
    modelDownloadProgress,

    // Computed
    displayImages,
    currentImage,

    // Actions
    loadLibrary,
    loadThumbnails,
    navigateNext,
    navigatePrev,
    getRating,
    setRating,
    buildSearchIndex,
    searchImages,
    clearSearch,
  };
});
