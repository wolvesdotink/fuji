import { shallowRef } from "vue";

/**
 * The keyboard cursor of the grid on screen. The cursor itself is the store's
 * `currentIndex` (the photo `G`, Enter and the rating keys act on, and the one
 * the viewer opens on); this is the grid's side of it, registered by
 * useGridCursor while ImageGrid or LibraryGrid is mounted. useKeyboardNav
 * reads the layout from it and useViewTransition asks it to show the cursor
 * after a return from the viewer. Routed through a module-level ref, like the
 * video player bus, because the keyboard handler lives in App, not under the
 * grid.
 */
export interface GridCursor {
  /** Column count as laid out now. */
  readonly columns: number;
  /** Rows that fit in the viewport (at least 1). */
  pageRows(): number;
  /**
   * Bring the cursor's card into view once pending list changes are applied.
   * `"follow"` (keyboard movement) scrolls the least amount that shows the
   * whole card and its ring. `"return"` (back from the viewer) keeps the
   * position when the card is already fully visible, and otherwise centres it
   * if it was off screen. Resolves once the card is mounted.
   */
  reveal(mode?: "follow" | "return"): Promise<void>;
  /** The cursor card's thumbnail container, if mounted: the morph's hero. */
  currentCard(): HTMLElement | null;
}

export const activeGridCursor = shallowRef<GridCursor | null>(null);

export type CursorKey =
  | "ArrowLeft"
  | "ArrowRight"
  | "ArrowUp"
  | "ArrowDown"
  | "PageUp"
  | "PageDown"
  | "Home"
  | "End";

export function isCursorKey(key: string): key is CursorKey {
  switch (key) {
    case "ArrowLeft":
    case "ArrowRight":
    case "ArrowUp":
    case "ArrowDown":
    case "PageUp":
    case "PageDown":
    case "Home":
    case "End":
      return true;
    default:
      return false;
  }
}

/**
 * Where a cursor key moves the cursor in a grid of `count` items laid out in
 * `columns` columns. Clamps at the ends, never wraps. Vertical moves keep the
 * column and stop at the first or last row; on the last, partial row that
 * means the last item when the column has no item below.
 */
export function cursorTarget(
  key: CursorKey,
  index: number,
  count: number,
  columns: number,
  pageRows: number
): number {
  const last = count - 1;
  const cols = Math.max(columns, 1);
  const at = Math.min(Math.max(index, 0), last);
  const row = Math.floor(at / cols);
  const lastRow = Math.floor(last / cols);
  const byRows = (n: number) =>
    Math.min(Math.min(Math.max(row + n, 0), lastRow) * cols + (at % cols), last);
  switch (key) {
    case "ArrowLeft":
      return Math.max(at - 1, 0);
    case "ArrowRight":
      return Math.min(at + 1, last);
    case "ArrowUp":
      return byRows(-1);
    case "ArrowDown":
      return byRows(1);
    case "PageUp":
      return byRows(-Math.max(pageRows, 1));
    case "PageDown":
      return byRows(Math.max(pageRows, 1));
    case "Home":
      return 0;
    case "End":
      return last;
  }
}
