import { defineStore } from "pinia";
import { ref, computed, shallowRef, watch } from "vue";
import { homeDir, join } from "@tauri-apps/api/path";
import type { LibraryImage, SearchResult } from "@/types";
import type { RatingFilter, MediaFilter } from "@/lib/libraryFilter";
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

  // Filters. Session state only (not persisted): a filter left on from a
  // previous session would make a library look half empty on launch.
  const ratingFilter = ref<RatingFilter>("all");
  const mediaFilter = ref<MediaFilter>("all");
  const isFiltered = computed(
    () => ratingFilter.value !== "all" || mediaFilter.value !== "all"
  );

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
  const searchScores = computed(() => {
    if (!searchResults.value) return new Map<string, number>();
    return new Map(searchResults.value.map((r) => [r.image_id, r.score]));
  });

  const displayImages = computed(() => {
    const rf = ratingFilter.value;
    const mf = mediaFilter.value;
    const sort = sortBy.value;
    const scores = searchResults.value ? searchScores.value : null;

    // Ratings are only read when the rating filter or the stars sort needs
    // them, so with neither active a rating change does not recompute the
    // list at all. In single view read the frozen snapshot (see
    // frozenRatings): rating the on-screen photo must neither re-sort it nor
    // filter it out from under the viewer. `?? ratings.value` short-circuits
    // in grid mode, so grid tracks live ratings.
    const needsRatings = rf !== "all" || (!scores && sort === "stars");
    const r = needsRatings ? (frozenRatings.value ?? ratings.value) : null;

    // One pass: the search scope, the media filter and the rating filter
    // together. This pass also produces the copy that gets sorted below, so
    // filtering costs no extra allocation over the old [...images] copy.
    const src = images.value;
    let list: LibraryImage[];
    if (!scores && rf === "all" && mf === "all") {
      list = src.slice();
    } else {
      list = [];
      for (let i = 0; i < src.length; i++) {
        const img = src[i];
        if (scores && !scores.has(img.id)) continue;
        if (mf !== "all" && img.media_type !== mf) continue;
        if (rf !== "all") {
          const stars = r!.get(img.file_path) ?? 0;
          if (rf === "unrated" ? stars > 0 : stars < rf) continue;
        }
        list.push(img);
      }
    }

    if (scores) {
      return list.sort(
        (a, b) => (scores.get(b.id) ?? 0) - (scores.get(a.id) ?? 0)
      );
    }

    // Newest first. Burst shots share a timestamp to the second, so ties fall
    // back to the camera's file numbering to keep the order they were shot in.
    const byName = (a: LibraryImage, b: LibraryImage) =>
      a.file_name < b.file_name ? 1 : a.file_name > b.file_name ? -1 : 0;
    const byCreated = (a: LibraryImage, b: LibraryImage) =>
      b.date_created - a.date_created || byName(a, b);

    switch (sort) {
      case "updated":
        list.sort((a, b) => b.date_modified - a.date_modified || byName(a, b));
        break;
      case "stars":
        list.sort((a, b) => {
          const rA = r!.get(a.file_path) ?? 0;
          const rB = r!.get(b.file_path) ?? 0;
          if (rA !== rB) return rB - rA;
          return byCreated(a, b);
        });
        break;
      case "created":
      default:
        list.sort(byCreated);
        break;
    }

    return list;
  });

  /**
   * Per-media histogram of ratings (index 0 = unrated, 1-5 = stars) over the
   * current search scope, ignoring the filters themselves. Feeds the counts
   * in the filter popover; only evaluated while something renders it.
   *
   * Live ratings, not the viewer's frozen snapshot: the counts predict what
   * clicking an option gives, and changing a filter refreshes the snapshot
   * to live ratings (see changeFilters).
   */
  const filterCounts = computed(() => {
    const r = ratings.value;
    const scores = searchResults.value ? searchScores.value : null;
    const photo = [0, 0, 0, 0, 0, 0];
    const video = [0, 0, 0, 0, 0, 0];
    for (const img of images.value) {
      if (scores && !scores.has(img.id)) continue;
      const stars = r.get(img.file_path) ?? 0;
      const bucket = stars > 5 ? 5 : stars < 0 ? 0 : stars;
      (img.media_type === "Video" ? video : photo)[bucket]++;
    }
    return { photo, video };
  });

  /** Media in the current scope (whole library or search results), before filters. */
  const unfilteredCount = computed(() => {
    if (!searchResults.value) return images.value.length;
    const scores = searchScores.value;
    let n = 0;
    for (const img of images.value) if (scores.has(img.id)) n++;
    return n;
  });

  const currentImage = computed(
    () => displayImages.value[currentIndex.value] ?? null
  );

  /**
   * After the list changed underneath us, point currentIndex back at the
   * photo that was current (`prevList[prevIndex]`). If that photo is no
   * longer listed, land on the nearest one that is, preferring the photos
   * that came after it.
   */
  function reanchor(
    prevList: LibraryImage[],
    prevIndex: number,
    list: LibraryImage[] = displayImages.value
  ) {
    const prev = prevList[prevIndex];
    if (list.length === 0 || !prev) {
      currentIndex.value = 0;
      return;
    }
    const same = list.indexOf(prev);
    if (same !== -1) {
      currentIndex.value = same;
      return;
    }
    const indexById = new Map<string, number>();
    for (let i = 0; i < list.length; i++) indexById.set(list[i].id, i);
    for (let d = 1; d < prevList.length; d++) {
      const after = prevList[prevIndex + d];
      const hitAfter = after && indexById.get(after.id);
      if (hitAfter !== undefined) {
        currentIndex.value = hitAfter;
        return;
      }
      const before = prevList[prevIndex - d];
      const hitBefore = before && indexById.get(before.id);
      if (hitBefore !== undefined) {
        currentIndex.value = hitBefore;
        return;
      }
    }
    currentIndex.value = 0;
  }

  // Snapshot ratings the instant we enter single view, and drop the snapshot
  // on the way back to grid. flush: "sync" is required: the snapshot must be
  // in place before displayImages re-evaluates in the same tick as the
  // viewMode flip, otherwise the first read would still see live ratings.
  //
  // Dropping the snapshot re-sorts and re-filters with live ratings, so a
  // photo rated below the rating filter in the viewer leaves the list here.
  // Re-anchor currentIndex so it keeps pointing at the photo that was on
  // screen (or its nearest surviving neighbour) rather than at whatever now
  // occupies its old slot.
  watch(
    viewMode,
    (mode) => {
      if (mode === "single") {
        frozenRatings.value = new Map(ratings.value);
        // Nothing to show (empty search or filters): the viewer would render
        // blank, so stay on the grid, which explains why it is empty.
        const len = displayImages.value.length;
        if (len === 0) viewMode.value = "grid";
        else if (currentIndex.value >= len) currentIndex.value = len - 1;
        return;
      }
      if (!frozenRatings.value) return;
      const prevList = displayImages.value;
      const prevIndex = currentIndex.value;
      frozenRatings.value = null;
      reanchor(prevList, prevIndex);
    },
    { flush: "sync" }
  );

  /**
   * Apply a filter change, keeping the current photo selected when it is
   * still listed (else its nearest surviving neighbour). In single view the
   * user just asked for a new list, so the ratings snapshot is refreshed to
   * live ratings first; otherwise a photo rated in the viewer would be
   * filtered by its old rating. If nothing matches, fall back to the grid,
   * which explains the empty result and offers to clear the filters.
   */
  function changeFilters(apply: () => void) {
    const prevList = displayImages.value;
    const prevIndex = currentIndex.value;
    apply();
    if (frozenRatings.value) frozenRatings.value = new Map(ratings.value);
    reanchor(prevList, prevIndex);
    if (viewMode.value === "single" && displayImages.value.length === 0) {
      viewMode.value = "grid";
    }
  }

  // Set by callers that pick the selection themselves while replacing the
  // list (new search, cleared search, new folder), so the reconciler below
  // only range-checks their choice instead of re-anchoring over it.
  let selectionReset = false;

  function resetSelection() {
    currentIndex.value = 0;
    selectionReset = true;
  }

  // The selection invariant, owned here: currentIndex is a valid index of
  // displayImages, or the list is empty and the view is the grid (a viewer
  // with no current image renders blank, and G/Esc, which act on the current
  // image, could not leave it).
  //
  // Filter changes and leaving the viewer re-anchor synchronously above;
  // this catches every other change to the list's membership or order:
  // ratings read from disk landing late, search results arriving or
  // clearing, a sort change, a new catalog. When the list changed and nobody
  // chose an index, follow the photo that was current (or its nearest
  // surviving neighbour), so the selection never silently points at
  // whatever slid into its old slot.
  //
  // flush "pre" (the default) batches: hydrating thousands of ratings is one
  // reconcile before the next render, not one per rating. Nothing user-driven
  // can run between the change and the flush.
  watch([displayImages, currentIndex], ([list, idx], [prevList, prevIdx]) => {
    const reset = selectionReset;
    selectionReset = false;
    if (list.length === 0) {
      currentIndex.value = 0;
      if (viewMode.value === "single") viewMode.value = "grid";
      return;
    }
    if (!reset && list !== prevList && idx === prevIdx && prevList) {
      reanchor(prevList, prevIdx, list);
    } else if (idx >= list.length || idx < 0) {
      currentIndex.value = list.length - 1;
    }
  });

  function setRatingFilter(f: RatingFilter) {
    if (f !== ratingFilter.value) changeFilters(() => (ratingFilter.value = f));
  }

  function setMediaFilter(f: MediaFilter) {
    if (f !== mediaFilter.value) changeFilters(() => (mediaFilter.value = f));
  }

  function clearFilters() {
    if (!isFiltered.value) return;
    changeFilters(() => {
      ratingFilter.value = "all";
      mediaFilter.value = "all";
    });
  }

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
  // Bumped per loadLibrary call so a slow ratings read for a folder the user
  // already left cannot apply its ratings to the new folder's images.
  let loadGeneration = 0;

  async function loadLibrary(destPath: string) {
    if (!destPath) return;
    const generation = ++loadGeneration;
    // Leave the viewer before the catalog changes. Going to grid also drops
    // the frozen ratings snapshot, which holds the previous folder's paths
    // and would otherwise filter and sort the new folder against them.
    viewMode.value = "grid";
    isLoading.value = true;
    try {
      const home = await homeDir();
      const indexCacheDir = await join(home, ".cache", "fuji-culler");
      const listed = await listLibraryImages(destPath, indexCacheDir);
      if (generation !== loadGeneration) return;
      viewMode.value = "grid";
      images.value = listed;
      thumbnailPaths.value = new Map();
      resetSelection();
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
            if (generation !== loadGeneration) return;
            // Build the id → image lookup once (O(n)). The previous
            // images.value.find() per returned rating was O(n) each, i.e.
            // O(n^2) across a full library of camera-set ratings.
            const hydrated: [string, number][] = [];
            const imageById = new Map(images.value.map((i) => [i.id, i]));
            const imageByStem = new Map(
              images.value.map((i) => [i.file_name.replace(/\.[^.]+$/, ""), i])
            );
            for (const [stem, rating] of Object.entries(fileRatings)) {
              const img = imageById.get(stem) ?? imageByStem.get(stem);
              if (img) {
                ratings.value.set(img.file_path, rating);
                hydrated.push([img.file_path, rating]);
              }
            }
            // Opened the viewer before the read finished: these are the
            // files' own ratings, not edits made in the viewer, so fold them
            // into the snapshot too (the reconciler then keeps the selection
            // valid). Viewer edits still waiting to be written keep theirs.
            const frozen = frozenRatings.value;
            if (frozen && hydrated.length > 0) {
              const next = new Map(frozen);
              for (const [path, rating] of hydrated) {
                if (!pendingWrites.has(path)) next.set(path, rating);
              }
              frozenRatings.value = next;
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
          if (progress.thumbnail_path) {
            thumbnailPaths.value.set(progress.image_id, progress.thumbnail_path);
          }
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
      resetSelection();
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
    resetSelection();
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
    ratingFilter,
    mediaFilter,
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
    isFiltered,
    filterCounts,
    unfilteredCount,

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
    setRatingFilter,
    setMediaFilter,
    clearFilters,
  };
});
