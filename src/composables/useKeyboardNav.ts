import { onMounted, onUnmounted } from "vue";
import { useAppStore } from "@/stores/app";
import { useGalleryStore } from "@/stores/gallery";
import { useLibraryStore } from "@/stores/library";
import { useViewTransition } from "@/composables/useViewTransition";
import {
  activeVideoPlayer,
  type VideoPlayerHandle,
} from "@/lib/videoPlayerBus";
import { activeLoupe } from "@/lib/loupeBus";
import { activeGridCursor, cursorTarget, isCursorKey } from "@/lib/gridCursor";

/**
 * The grid card's thumbnail container for the item at `index` in the grid's
 * current list, used as the hero of the grid→viewer morph. Looked up by index
 * rather than image id: ids are file stems, which repeat across folders in a
 * recursive library scan. The grids are windowed, so the card is only in the
 * DOM when its row is near the viewport; null otherwise, and the transition
 * falls back to a plain cross-fade.
 */
function gridThumbnail(gridSelector: string, index: number): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    `${gridSelector} .thumbnail-container[data-index="${index}"]`
  );
}

/** A focused control reached with Tab, which Enter and Space activate. */
function isControl(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    target.closest("button, a[href], select, [role='button']") !== null
  );
}

/** What the grid-mode keys need from the camera or library store. */
interface GridNav {
  /** The grid's container class, for finding the hero card. */
  selector: string;
  count(): number;
  index(): number;
  setIndex(index: number): void;
  /** Id of the photo under the cursor, or null for an empty grid. */
  currentId(): string | null;
  /** Switch to the viewer (inside the view transition). */
  openViewer(): void;
}

export function useKeyboardNav() {
  const appStore = useAppStore();
  const galleryStore = useGalleryStore();
  const libraryStore = useLibraryStore();
  const { startTransition } = useViewTransition();

  /**
   * Player keys, routed through the videoPlayerBus rather than element focus —
   * the user normally arrives on a clip via the arrow keys, so the <video> is
   * never focused and a focus-based scheme would leave the player unreachable.
   * Returns true when the key was consumed.
   *
   * Navigation (arrows) and rating (0-5) are deliberately absent: culling stays
   * the primary flow, so seeking gets its own keys and rating is never hijacked.
   */
  function handleVideoKeys(e: KeyboardEvent, player: VideoPlayerHandle) {
    switch (e.key) {
      case " ":
      case "k":
      case "K":
        e.preventDefault();
        player.togglePlay();
        return true;
      case "j":
      case "J":
        e.preventDefault();
        player.seekBy(-5);
        return true;
      case "l":
      case "L":
        e.preventDefault();
        player.seekBy(5);
        return true;
      // Frame stepping for judging sharpness on a clip. stepFrame pauses first,
      // so these work whether or not playback is running.
      case ",":
        e.preventDefault();
        player.stepFrame(-1);
        return true;
      case ".":
        e.preventDefault();
        player.stepFrame(1);
        return true;
      case "f":
      case "F":
        e.preventDefault();
        player.toggleFullscreen();
        return true;
      // Safe to claim: toggleMarkForCompare already no-ops for videos, so M
      // has nothing else to do here.
      case "m":
      case "M":
        e.preventDefault();
        player.toggleMute();
        return true;
      default:
        return false;
    }
  }

  /**
   * A modal owns the keyboard: the camera import prompt (it takes Enter and
   * Esc itself) or the full-screen import screen.
   */
  function overlayOwnsKeyboard(): boolean {
    return (
      document.querySelector('[aria-modal="true"]') !== null ||
      (galleryStore.importState !== "idle" && galleryStore.importScreenVisible)
    );
  }

  /** Bring the grid's cursor into view (after a pending list change). */
  function revealCursor() {
    return activeGridCursor.value?.reveal() ?? Promise.resolve();
  }

  /**
   * Open the photo under the cursor with the hero morph. Its card is
   * scrolled into view first (it can be off screen after a mouse scroll), so
   * the morph starts from it instead of degrading to a cross-fade.
   */
  async function openFromGrid(nav: GridNav) {
    await revealCursor();
    const id = nav.currentId();
    if (!id) return;
    const sourceEl = gridThumbnail(nav.selector, nav.index());
    startTransition(id, nav.openViewer, sourceEl);
  }

  /**
   * Keys shared by both grids: the keyboard cursor (the store's
   * currentIndex, drawn on its card) moves in two dimensions and Enter or G
   * opens it. Returns true when the key is done with here, including keys
   * deliberately left to the system; false hands it to the mode's own keys.
   */
  function handleGridKeys(e: KeyboardEvent, nav: GridNav): boolean {
    if (overlayOwnsKeyboard()) return true;
    // Cmd/Ctrl/Alt combinations belong to the system and the webview.
    if (e.metaKey || e.ctrlKey || e.altKey) return true;
    if ((e.key === "Enter" || e.key === " ") && isControl(e.target)) return true;
    const count = nav.count();
    if (count === 0) return false;

    if (isCursorKey(e.key)) {
      e.preventDefault();
      const cursor = activeGridCursor.value;
      nav.setIndex(
        cursorTarget(e.key, nav.index(), count, cursor?.columns ?? 1, cursor?.pageRows() ?? 1)
      );
      // Also when the cursor didn't move: it may be off screen after a
      // mouse scroll.
      void revealCursor();
      return true;
    }
    switch (e.key) {
      case "Enter":
      case "g":
      case "G":
        e.preventDefault();
        if (!e.repeat) void openFromGrid(nav);
        return true;
    }
    return false;
  }

  const libraryGrid: GridNav = {
    selector: ".library-grid",
    count: () => libraryStore.displayImages.length,
    index: () => libraryStore.currentIndex,
    setIndex: (i) => (libraryStore.currentIndex = i),
    currentId: () => libraryStore.currentImage?.id ?? null,
    openViewer: () => (libraryStore.viewMode = "single"),
  };

  const cameraGrid: GridNav = {
    selector: ".image-grid",
    count: () => galleryStore.images.length,
    index: () => galleryStore.currentIndex,
    setIndex: (i) => (galleryStore.currentIndex = i),
    currentId: () => galleryStore.currentImage?.id ?? null,
    openViewer: () => (galleryStore.viewMode = "single"),
  };

  /** Library grid: shared grid keys, plus rating the photo under the cursor. */
  function handleLibraryGridKeys(e: KeyboardEvent): boolean {
    if (handleGridKeys(e, libraryGrid)) return true;
    if (/^[0-5]$/.test(e.key)) {
      e.preventDefault();
      const img = libraryStore.currentImage;
      if (img) {
        // Rate only, like the viewer. Under the Stars sort (or a rating
        // filter) the list changes live in the grid; the store keeps the
        // cursor on this photo (or its nearest neighbour) and the grid
        // scrolls to keep it on screen.
        libraryStore.setRating(img.file_path, parseInt(e.key));
        void revealCursor();
      }
      return true;
    }
    return false;
  }

  /**
   * Camera grid: shared grid keys; rating (then advancing), Space and M act
   * as in the viewer, and the cursor is brought into view afterwards.
   */
  function handleCameraGridKeys(e: KeyboardEvent): boolean {
    if (handleGridKeys(e, cameraGrid)) return true;
    if (/^[0-5]$/.test(e.key)) {
      e.preventDefault();
      galleryStore.rateAndAdvance(parseInt(e.key));
      void revealCursor();
      return true;
    }
    switch (e.key) {
      case " ":
        e.preventDefault();
        galleryStore.jumpToNextUnreviewed();
        void revealCursor();
        return true;
      case "m":
      case "M": {
        e.preventDefault();
        const img = galleryStore.currentImage;
        if (img) galleryStore.toggleMarkForCompare(img.id);
        void revealCursor();
        return true;
      }
    }
    return false;
  }

  function handleKeydown(e: KeyboardEvent) {
    // Native-controls fallback (VideoPlayer's `nativeControls` prop): when the
    // browser's own control bar owns the element, let it keep Space and the
    // arrow keys. The custom player has `controls` off and routes keys through
    // the bus instead, so it never matches here.
    if (
      e.target instanceof HTMLVideoElement &&
      e.target.controls &&
      e.key !== "Escape"
    ) {
      return;
    }

    // Don't handle if user is typing in an input
    if (
      e.target instanceof HTMLInputElement ||
      e.target instanceof HTMLTextAreaElement
    ) {
      // Allow Escape to blur search input
      if (e.key === "Escape") {
        (e.target as HTMLElement).blur();
        e.preventDefault();
      }
      return;
    }

    if (appStore.appMode === "library") {
      handleLibraryKeys(e);
    } else {
      handleCameraKeys(e);
    }
  }

  function handleLibraryKeys(e: KeyboardEvent) {
    if (libraryStore.viewMode === "grid" && handleLibraryGridKeys(e)) return;

    const player = activeVideoPlayer.value;
    if (
      player &&
      libraryStore.viewMode === "single" &&
      libraryStore.currentImage?.media_type === "Video" &&
      handleVideoKeys(e, player)
    ) {
      return;
    }

    switch (e.key) {
      case "ArrowLeft":
        e.preventDefault();
        libraryStore.navigatePrev();
        break;
      case "ArrowRight":
        e.preventDefault();
        libraryStore.navigateNext();
        break;
      // Star ratings 1-5 (single-image view only)
      case "1":
      case "2":
      case "3":
      case "4":
      case "5": {
        if (libraryStore.viewMode !== "single") break;
        e.preventDefault();
        const libImg = libraryStore.currentImage;
        if (libImg) {
          libraryStore.setRating(libImg.file_path, parseInt(e.key));
        }
        break;
      }
      case "0": {
        if (libraryStore.viewMode !== "single") break;
        e.preventDefault();
        const libImg0 = libraryStore.currentImage;
        if (libImg0) {
          libraryStore.setRating(libImg0.file_path, 0);
        }
        break;
      }
      case "g":
      case "G": {
        e.preventDefault();
        const libImg = libraryStore.currentImage;
        if (libImg) {
          // Find the card's container for imperative tagging (grid→single only)
          const sourceEl = libraryStore.viewMode === "grid"
            ? gridThumbnail(".library-grid", libraryStore.currentIndex)
            : null;
          // Leaving the viewer zoomed: drop to fit first so the morph back
          // to the grid card starts from the whole frame, not a 100% crop.
          activeLoupe.value?.reset();
          startTransition(libImg.id, () => {
            libraryStore.viewMode =
              libraryStore.viewMode === "grid" ? "single" : "grid";
          }, sourceEl);
        }
        break;
      }
      case "z":
      case "Z":
        if (libraryStore.viewMode !== "single") break;
        e.preventDefault();
        activeLoupe.value?.toggle();
        break;
      case "/":
        e.preventDefault();
        // Focus the search input
        document.querySelector<HTMLInputElement>(".search-input")?.focus();
        break;
      case "Escape":
        e.preventDefault();
        // Zoomed in: the first Esc only returns to fit.
        if (libraryStore.viewMode === "single" && activeLoupe.value?.isZoomed()) {
          activeLoupe.value.reset();
        } else if (libraryStore.viewMode === "single") {
          const libEscImg = libraryStore.currentImage;
          if (libEscImg) {
            startTransition(libEscImg.id, () => {
              libraryStore.viewMode = "grid";
            });
          }
        } else if (libraryStore.searchQuery) {
          libraryStore.clearSearch();
        }
        break;
    }
  }

  function handleCameraKeys(e: KeyboardEvent) {
    if (galleryStore.viewMode === "grid" && handleCameraGridKeys(e)) return;

    // In compare mode, arrow keys / number keys / G would be confusing
    // (there's no "current image" to navigate) — only Esc, C, and M are
    // meaningful. Handle those up front and early-return.
    if (galleryStore.viewMode === "compare") {
      if (e.key === "Escape" || e.key === "c" || e.key === "C") {
        e.preventDefault();
        galleryStore.viewMode = "single";
        return;
      }
      if (e.key === "m" || e.key === "M") {
        // Fall through to the M handler below — it toggles the mark on
        // `currentImage` (the image focused before compare was opened).
      } else {
        return;
      }
    }

    // On a clip in single view the player claims Space/J/K/L/,/./F/M. Compare
    // mode is excluded by the viewMode check (it never holds videos anyway).
    const player = activeVideoPlayer.value;
    if (
      player &&
      galleryStore.viewMode === "single" &&
      galleryStore.currentImage?.media_type === "Video" &&
      handleVideoKeys(e, player)
    ) {
      return;
    }

    switch (e.key) {
      case "ArrowLeft":
        e.preventDefault();
        galleryStore.navigatePrev();
        break;
      case "ArrowRight":
        e.preventDefault();
        galleryStore.navigateNext();
        break;
      // Star ratings 1-5
      case "1":
        e.preventDefault();
        galleryStore.rateAndAdvance(1);
        break;
      case "2":
        e.preventDefault();
        galleryStore.rateAndAdvance(2);
        break;
      case "3":
        e.preventDefault();
        galleryStore.rateAndAdvance(3);
        break;
      case "4":
        e.preventDefault();
        galleryStore.rateAndAdvance(4);
        break;
      case "5":
        e.preventDefault();
        galleryStore.rateAndAdvance(5);
        break;
      case "0":
        e.preventDefault();
        galleryStore.rateAndAdvance(0);
        break;
      case " ":
        e.preventDefault();
        galleryStore.jumpToNextUnreviewed();
        break;
      case "g":
      case "G": {
        e.preventDefault();
        const camImg = galleryStore.currentImage;
        if (camImg) {
          const sourceEl = galleryStore.viewMode === "grid"
            ? gridThumbnail(".image-grid", galleryStore.currentIndex)
            : null;
          activeLoupe.value?.reset();
          startTransition(camImg.id, () => {
            galleryStore.viewMode =
              galleryStore.viewMode === "grid" ? "single" : "grid";
          }, sourceEl);
        }
        break;
      }
      case "z":
      case "Z":
        if (galleryStore.viewMode !== "single") break;
        e.preventDefault();
        activeLoupe.value?.toggle();
        break;
      case "Escape":
        // Compare mode handled Esc above; in single view it only unzooms.
        if (galleryStore.viewMode === "single" && activeLoupe.value?.isZoomed()) {
          e.preventDefault();
          activeLoupe.value.reset();
        }
        break;
      // Compare mode
      case "m":
      case "M": {
        e.preventDefault();
        const markImg = galleryStore.currentImage;
        if (markImg) {
          galleryStore.toggleMarkForCompare(markImg.id);
        }
        break;
      }
      case "c":
      case "C": {
        e.preventDefault();
        // Already handled above when in compare mode. Here only the
        // single/grid → compare transition matters.
        if (galleryStore.markedForCompare.size >= 2) {
          galleryStore.openCompareView();
        }
        break;
      }
    }
  }

  onMounted(() => {
    window.addEventListener("keydown", handleKeydown);
  });

  onUnmounted(() => {
    window.removeEventListener("keydown", handleKeydown);
  });
}
