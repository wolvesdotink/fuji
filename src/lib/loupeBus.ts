import { shallowRef } from "vue";

/**
 * Imperative handle the on-screen single-image viewer publishes so the global
 * keyboard layer can drive its loupe (`Z`, `Esc`). Same pattern and rationale
 * as `videoPlayerBus`: at most one viewer is mounted at a time, and the
 * keyboard handler lives at the app root.
 */
export interface LoupeHandle {
  /** Fit ↔ 100%, anchored on the centre of the view. */
  toggle(): void;
  /** Back to fit. Synchronous, so a view transition started right after
   *  captures the fitted frame. */
  reset(): void;
  /** True while zoomed in, or while a zoom waits for the full-res image. */
  isZoomed(): boolean;
}

export const activeLoupe = shallowRef<LoupeHandle | null>(null);

export function registerLoupe(handle: LoupeHandle) {
  activeLoupe.value = handle;
}

export function unregisterLoupe(handle: LoupeHandle) {
  if (activeLoupe.value === handle) activeLoupe.value = null;
}
