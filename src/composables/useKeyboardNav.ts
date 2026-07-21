import { onMounted, onUnmounted } from "vue";
import { useAppStore } from "@/stores/app";
import { useGalleryStore } from "@/stores/gallery";
import { useLibraryStore } from "@/stores/library";
import { useViewTransition } from "@/composables/useViewTransition";

export function useKeyboardNav() {
  const appStore = useAppStore();
  const galleryStore = useGalleryStore();
  const libraryStore = useLibraryStore();
  const { startTransition } = useViewTransition();

  function handleKeydown(e: KeyboardEvent) {
    // Let native video controls own Space and arrow keys for play/pause and
    // seeking. Escape still bubbles into the app's normal back behavior.
    if (e.target instanceof HTMLVideoElement && e.key !== "Escape") {
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
