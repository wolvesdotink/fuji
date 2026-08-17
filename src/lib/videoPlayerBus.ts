import { shallowRef } from "vue";

/**
 * Imperative handle a mounted VideoPlayer publishes so the global keyboard
 * layer can drive it.
 *
 * Why a module-scoped slot instead of provide/inject or store state: both
 * viewers only ever show one item at a time, so there is at most one player
 * on screen. The keyboard handler lives in a composable mounted at the app
 * root — plumbing a ref down to it would mean threading it through two stores
 * and three components for a value that is, in practice, a singleton.
 */
export interface VideoPlayerHandle {
  /** Play if paused, pause if playing. */
  togglePlay(): void;
  pause(): void;
  /** Seek relative to the current position, clamped to the clip. */
  seekBy(seconds: number): void;
  /** Nudge by a single (nominal) frame; pauses first. `dir` is -1 or 1. */
  stepFrame(dir: number): void;
  toggleMute(): void;
  toggleFullscreen(): void;
  isPlaying(): boolean;
}

/** The player currently on screen, or null when the item isn't a video. */
export const activeVideoPlayer = shallowRef<VideoPlayerHandle | null>(null);

export function registerVideoPlayer(handle: VideoPlayerHandle) {
  activeVideoPlayer.value = handle;
}

export function unregisterVideoPlayer(handle: VideoPlayerHandle) {
  // Only clear if we're still the registered player. Vue mounts the incoming
  // component before unmounting the outgoing one when a `:key` changes, so a
  // naive `= null` here would erase the player that just replaced us.
  if (activeVideoPlayer.value === handle) {
    activeVideoPlayer.value = null;
  }
}
