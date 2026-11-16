import { ref, nextTick } from "vue";
import { activeGridCursor } from "@/lib/gridCursor";

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
   *
   * The grids are windowed, so either card may not be mounted (the source
   * card is off-screen, or the user navigated far from the restored scroll
   * position in the viewer). Then only one side has a hero and the browser
   * falls back to a plain cross-fade for it.
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
      await restoreGridScroll();
      cleanup(sourceEl);
      return;
    }

    const transition = document.startViewTransition(async () => {
      callback();
      await nextTick();

      // Restore first: the windowed grid only mounts the rows around the
      // restored position, and the hero card must be among them before we
      // look for it below.
      await restoreGridScroll();

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

  async function restoreGridScroll() {
    const gridContainer = document.querySelector(GRID_CONTAINER_SELECTOR);
    if (gridContainer && savedGridScrollTop !== null) {
      gridContainer.scrollTop = savedGridScrollTop;
      // The browser fires the scroll event on its next frame, after the
      // view-transition snapshot. Dispatch it now so VirtualGrid mounts the
      // rows for the restored position, then let Vue patch the DOM.
      gridContainer.dispatchEvent(new Event("scroll"));
      await nextTick();
    }
    // The viewer may have moved far from the photo the grid was left on.
    // The restored position stands while the current photo is fully visible
    // in it; otherwise the grid brings it into view, so the cursor is on
    // screen and the hero card is mounted before the snapshot.
    await activeGridCursor.value?.reveal("return");
  }

  return { activeTransitionId, startTransition };
}
