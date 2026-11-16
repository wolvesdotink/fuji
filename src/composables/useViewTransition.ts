import { ref, nextTick } from "vue";
import { activeGridCursor } from "@/lib/gridCursor";
import type { GridPlace } from "@/components/VirtualGrid.vue";

// Module-level shared state. Transitions can overlap (closing the viewer
// while the opening morph still runs), so each one gets a token and cleans
// up only what it still owns: the shared id if no later transition has set
// it, and the hero names it put on elements.
const activeTransitionId = ref<string | null>(null);
let latestToken = 0;
const heroOwner = new WeakMap<HTMLElement, number>();

function tagHero(el: HTMLElement, token: number) {
  el.style.setProperty("view-transition-name", "hero-image");
  heroOwner.set(el, token);
}

// The grid's place when the viewer replaced it, in the grid's own terms
// (anchored item and offset), put back when returning to the grid. A raw
// scrollTop would land elsewhere if the geometry changed meanwhile (a window
// resize in the viewer), and a later re-measure would move it.
let savedGridPlace: GridPlace | null = null;

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
    // Leaving a grid: remember its place.
    const grid = activeGridCursor.value;
    if (grid) savedGridPlace = grid.capturePlace();

    const token = ++latestToken;

    // Set reactive id — needed when cards mount in the new state (single→grid)
    activeTransitionId.value = imageId;

    // Imperatively tag the source element (grid→single direction).
    // This bypasses Vue's async style patching entirely.
    if (sourceEl) tagHero(sourceEl, token);

    await nextTick();

    let destEl: HTMLElement | null = null;

    if (!document.startViewTransition) {
      callback();
      await nextTick();
      await restoreGridScroll();
      cleanup(token, sourceEl);
      return;
    }

    const transition = document.startViewTransition(async () => {
      callback();
      await nextTick();

      // Restore first: the windowed grid only mounts the rows around the
      // restored position, and the hero card must be among them before we
      // look for it below.
      await restoreGridScroll();
      destEl = await tagGridDestination(token);

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
      cleanup(token, sourceEl, destEl);
    }
  }

  /**
   * Viewer → grid: make the cursor's card the hero. The destination is only
   * known now: leaving the viewer can move the selection (the library store
   * re-anchors it when the photo just rated no longer passes the filters),
   * so the id the transition started with may have no card. Tagging by the
   * cursor's index also avoids a second card with the same file stem.
   * Returns the tagged element, or null when not returning to a grid or the
   * card is not mounted (the morph then falls back to a cross-fade).
   */
  async function tagGridDestination(token: number): Promise<HTMLElement | null> {
    const cursor = activeGridCursor.value;
    if (!cursor) return null;
    // Drop the id-based tag the cards may have picked up on mount.
    if (token === latestToken) activeTransitionId.value = null;
    await nextTick();
    const el = cursor.currentCard();
    if (el) tagHero(el, token);
    return el;
  }

  /**
   * After a transition finished, was skipped by a later one or failed:
   * remove the hero names it still owns, and the shared id unless a later
   * transition has taken it over.
   */
  function cleanup(token: number, ...els: (HTMLElement | null | undefined)[]) {
    for (const el of els) {
      if (el && heroOwner.get(el) === token) {
        el.style.removeProperty("view-transition-name");
        heroOwner.delete(el);
      }
    }
    if (token === latestToken) activeTransitionId.value = null;
  }

  async function restoreGridScroll() {
    const grid = activeGridCursor.value;
    if (!grid) return; // not returning to a grid
    // Put the grid's place back and let Vue mount its rows before the
    // view-transition snapshot. A place from the other grid doesn't apply.
    if (savedGridPlace && grid.restorePlace(savedGridPlace)) await nextTick();
    // The viewer may have moved far from the photo the grid was left on.
    // The restored position stands while the current photo is fully visible
    // in it; otherwise the grid brings it into view, so the cursor is on
    // screen and the hero card is mounted before the snapshot.
    await grid.reveal("return");
  }

  return { activeTransitionId, startTransition };
}
