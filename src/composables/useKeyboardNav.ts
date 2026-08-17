import { onMounted, onUnmounted } from "vue";
import { useAppStore } from "@/stores/app";
import { useGalleryStore } from "@/stores/gallery";
import { useLibraryStore } from "@/stores/library";
import { useViewTransition } from "@/composables/useViewTransition";
import {
  activeVideoPlayer,
  type VideoPlayerHandle,
} from "@/lib/videoPlayerBus";

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
            ? document.querySelector<HTMLElement>(`.library-grid .thumbnail-container:nth-child(${libraryStore.currentIndex + 1})`)
              ?? document.querySelectorAll<HTMLElement>(".library-grid .thumbnail-container")[libraryStore.currentIndex]
            : null;
          startTransition(libImg.id, () => {
            libraryStore.viewMode =
              libraryStore.viewMode === "grid" ? "single" : "grid";
          }, sourceEl);
        }
        break;
      }
      case "/":
        e.preventDefault();
        // Focus the search input
        document.querySelector<HTMLInputElement>(".search-input")?.focus();
        break;
      case "Escape":
        e.preventDefault();
        if (libraryStore.viewMode === "single") {
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
            ? document.querySelectorAll<HTMLElement>(".image-grid .thumbnail-container")[galleryStore.currentIndex]
            : null;
          startTransition(camImg.id, () => {
            galleryStore.viewMode =
              galleryStore.viewMode === "grid" ? "single" : "grid";
          }, sourceEl);
        }
        break;
      }
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
