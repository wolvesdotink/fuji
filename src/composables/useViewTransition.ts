import { ref, nextTick } from "vue";

// Module-level shared state — only one transition at a time
const activeTransitionId = ref<string | null>(null);

// Persisted grid scroll position so we can restore it when returning from the viewer
let savedGridScrollTop: number | null = null;

const GRID_CONTAINER_SELECTOR =
  ".image-grid-container, .library-grid-container";

export function useViewTransition() {
  /**
   * Wrap a view-mode change in a View Transition.
   *
   * Grid→Single: `sourceEl` (the card's .thumbnail-container) is tagged
   * imperatively before the old-state capture — Vue's reactive binding is
   * too slow.
   *
   * Single→Grid: `activeTransitionId` is set so the card that mounts fresh
   * in the new state picks up `view-transition-name` via its reactive
   * `:style` binding.
   */
  async function startTransition(
    imageId: string,
    callback: () => void,
    sourceEl?: HTMLElement | null
  ): Promise<void> {
    // Save grid scroll position before any transition (if grid is visible)
    const gridContainer = document.querySelector(GRID_CONTAINER_SELECTOR);
    if (gridContainer) {
      savedGridScrollTop = gridContainer.scrollTop;
    }

    // Set reactive id — needed when cards mount in the new state (single→grid)
    activeTransitionId.value = imageId;

    // Imperatively tag the source element (grid→single direction).
    // This bypasses Vue's async style patching entirely.
    if (sourceEl) {
      sourceEl.style.setProperty("view-transition-name", "hero-image");
    }

    await nextTick();

    if (!document.startViewTransition) {
      callback();
      await nextTick();
      restoreGridScroll();
      cleanup(sourceEl);
      return;
    }

    const transition = document.startViewTransition(async () => {
      callback();
      await nextTick();

      // Wait for the newly-mounted hero image to be paint-ready before the
      // new-state snapshot is captured. Matches either grid→viewer
      // (.preview-image inside the viewer's .image-container) or viewer→grid
      // (the card's thumbnail nested inside the freshly-tagged container).
      const heroImg = document.querySelector<HTMLImageElement>(
        '.image-container img.preview-image, ' +
        '[style*="view-transition-name: hero-image"] img'
      );
      if (heroImg && heroImg.src) {
        await Promise.race([
          heroImg.decode().catch(() => {}),
          new Promise<void>((r) => setTimeout(r, 80)),
        ]);
      }

      restoreGridScroll();
    });

    try {
      await transition.finished;
    } catch {
      // Transition was skipped or aborted — OK
    } finally {
      cleanup(sourceEl);
    }
  }

  function cleanup(sourceEl?: HTMLElement | null) {
    if (sourceEl) {
      sourceEl.style.removeProperty("view-transition-name");
    }
    activeTransitionId.value = null;
  }

  function restoreGridScroll() {
    if (savedGridScrollTop === null) return;
    const gridContainer = document.querySelector(GRID_CONTAINER_SELECTOR);
    if (gridContainer) {
      gridContainer.scrollTop = savedGridScrollTop;
    }
  }

  return { activeTransitionId, startTransition };
}
