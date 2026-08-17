<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  registerVideoPlayer,
  unregisterVideoPlayer,
  type VideoPlayerHandle,
} from "@/lib/videoPlayerBus";

const props = defineProps<{
  src: string;
  poster?: string;
  /** Escape hatch: hand playback back to the browser's own control bar. */
  nativeControls?: boolean;
}>();

// Volume is the one player setting worth remembering across clips — resetting
// to 100% on every video is the single most annoying default while culling.
const VOLUME_KEY = "fuji-culler.video-volume";
const MUTED_KEY = "fuji-culler.video-muted";

const RATES = [0.5, 1, 1.5, 2];

// HTMLVideoElement exposes no frame rate, so frame stepping uses a nominal
// 25 fps. It's a "nudge slightly" control for judging sharpness, not an edit
// decision, so being a frame off on 30/50p footage doesn't matter.
const FRAME_STEP_SECONDS = 1 / 25;

const CONTROLS_HIDE_MS = 2000;

const wrapper = ref<HTMLElement | null>(null);
const video = ref<HTMLVideoElement | null>(null);
const seekBar = ref<HTMLElement | null>(null);

const isPlaying = ref(false);
const currentTime = ref(0);
const duration = ref(0);
const volume = ref(1);
const muted = ref(false);
const rate = ref(1);
const loop = ref(false);
const isFullscreen = ref(false);
const rateMenuOpen = ref(false);
const controlsVisible = ref(true);
const scrubbing = ref(false);
/** Pointer position over the seek bar as 0..1, or null when not hovering. */
const hoverFraction = ref<number | null>(null);
/** Short-lived centre overlay confirming a play/pause toggle. */
const flash = ref<"play" | "pause" | null>(null);
const bufferedRanges = ref<{ left: number; width: number }[]>([]);

const playedPercent = computed(() =>
  duration.value > 0 ? (currentTime.value / duration.value) * 100 : 0
);

let hideHandle: number | undefined;
let flashHandle: number | undefined;

// --- Persistence -----------------------------------------------------------

function persistVolume() {
  try {
    localStorage.setItem(VOLUME_KEY, String(volume.value));
    localStorage.setItem(MUTED_KEY, muted.value ? "true" : "false");
  } catch {
    // Private-mode / quota failures are not worth surfacing.
  }
}

function restoreVolume() {
  // Note: Number(null) === 0, so a missing key must be checked before
  // coercion or a first run would restore to silence instead of full volume.
  const raw = localStorage.getItem(VOLUME_KEY);
  const stored = raw === null ? NaN : Number(raw);
  volume.value = Number.isFinite(stored) && stored >= 0 && stored <= 1 ? stored : 1;
  muted.value = localStorage.getItem(MUTED_KEY) === "true";
}

// --- Playback --------------------------------------------------------------

function flashIcon(kind: "play" | "pause") {
  flash.value = kind;
  if (flashHandle !== undefined) clearTimeout(flashHandle);
  flashHandle = window.setTimeout(() => (flash.value = null), 450);
}

function togglePlay() {
  const v = video.value;
  if (!v) return;
  if (v.paused) {
    flashIcon("play");
    // A rejected play() (autoplay policy, src not yet decodable) is expected,
    // not exceptional — swallow it rather than leaking an unhandled rejection.
    v.play().catch(() => {});
  } else {
    v.pause();
    flashIcon("pause");
  }
  revealControls();
}

function seekBy(seconds: number) {
  const v = video.value;
  if (!v) return;
  const max = Number.isFinite(v.duration) ? v.duration : v.currentTime;
  v.currentTime = Math.min(Math.max(v.currentTime + seconds, 0), max);
  currentTime.value = v.currentTime;
  revealControls();
}

function stepFrame(dir: number) {
  video.value?.pause();
  seekBy(dir * FRAME_STEP_SECONDS);
}

function setRate(next: number) {
  rate.value = next;
  if (video.value) video.value.playbackRate = next;
  rateMenuOpen.value = false;
  revealControls();
}

function toggleLoop() {
  loop.value = !loop.value;
  revealControls();
}

function setVolume(next: number) {
  const clamped = Math.min(Math.max(next, 0), 1);
  volume.value = clamped;
  // Dragging up from silence is an unmute in every player people already use.
  if (clamped > 0 && muted.value) muted.value = false;
  const v = video.value;
  if (v) {
    v.volume = clamped;
    v.muted = muted.value;
  }
  persistVolume();
}

function toggleMute() {
  muted.value = !muted.value;
  if (video.value) video.value.muted = muted.value;
  persistVolume();
  revealControls();
}

function toggleFullscreen() {
  // Fullscreen the wrapper, not the <video>: the element itself would show
  // the native control bar and hide ours.
  const el = wrapper.value;
  if (!el) return;
  const request = document.fullscreenElement
    ? document.exitFullscreen()
    : el.requestFullscreen();
  request.catch(() => {});
  revealControls();
}

function onFullscreenChange() {
  isFullscreen.value = document.fullscreenElement === wrapper.value;
}

// --- Media element events --------------------------------------------------

function syncDuration() {
  const d = video.value?.duration ?? 0;
  duration.value = Number.isFinite(d) ? d : 0;
}

function onTimeUpdate() {
  const v = video.value;
  if (!v) return;
  // While dragging, the pointer owns the playhead — a lagging timeupdate would
  // yank the thumb back under the cursor.
  if (!scrubbing.value) currentTime.value = v.currentTime;
  updateBuffered();
}

function updateBuffered() {
  const v = video.value;
  if (!v || !Number.isFinite(v.duration) || v.duration <= 0) {
    bufferedRanges.value = [];
    return;
  }
  const ranges: { left: number; width: number }[] = [];
  for (let i = 0; i < v.buffered.length; i++) {
    const start = v.buffered.start(i);
    const end = v.buffered.end(i);
    ranges.push({
      left: (start / v.duration) * 100,
      width: ((end - start) / v.duration) * 100,
    });
  }
  bufferedRanges.value = ranges;
}

function onPlay() {
  isPlaying.value = true;
  revealControls();
}

function onPause() {
  isPlaying.value = false;
  revealControls();
}

// --- Seek bar interaction --------------------------------------------------

function fractionFromEvent(e: PointerEvent): number {
  const el = seekBar.value;
  if (!el) return 0;
  const rect = el.getBoundingClientRect();
  if (rect.width === 0) return 0;
  return Math.min(Math.max((e.clientX - rect.left) / rect.width, 0), 1);
}

function seekToFraction(fraction: number) {
  const v = video.value;
  if (!v || duration.value <= 0) return;
  v.currentTime = fraction * duration.value;
  currentTime.value = v.currentTime;
}

function onSeekPointerDown(e: PointerEvent) {
  const el = seekBar.value;
  if (!el) return;
  scrubbing.value = true;
  // Capture so the drag keeps tracking once the pointer leaves the thin bar.
  el.setPointerCapture(e.pointerId);
  seekToFraction(fractionFromEvent(e));
}

function onSeekPointerMove(e: PointerEvent) {
  hoverFraction.value = fractionFromEvent(e);
  if (scrubbing.value) seekToFraction(hoverFraction.value);
}

function onSeekPointerUp(e: PointerEvent) {
  if (!scrubbing.value) return;
  scrubbing.value = false;
  seekBar.value?.releasePointerCapture(e.pointerId);
  revealControls();
}

// --- Control bar auto-hide -------------------------------------------------

function revealControls() {
  controlsVisible.value = true;
  if (hideHandle !== undefined) clearTimeout(hideHandle);
  hideHandle = undefined;
  // Nothing to hide from: a paused clip, an open menu or an active drag all
  // mean the user is looking at the controls right now.
  if (!isPlaying.value || rateMenuOpen.value || scrubbing.value) return;
  hideHandle = window.setTimeout(
    () => (controlsVisible.value = false),
    CONTROLS_HIDE_MS
  );
}

function onWrapperLeave() {
  rateMenuOpen.value = false;
  if (isPlaying.value) controlsVisible.value = false;
}

/**
 * Keep DOM focus off the control bar. A focused <button> would activate a
 * second time on Space, on top of the global handler's play/pause, and a
 * focused control makes the global key handler bail out entirely. Suppressing
 * the mousedown default skips focus (and text selection) while still letting
 * the click through. The volume slider opts out — it needs its native drag.
 */
function onControlMouseDown(e: MouseEvent) {
  if (!(e.target instanceof HTMLInputElement)) e.preventDefault();
}

function onVolumeInput(e: Event) {
  setVolume(Number((e.target as HTMLInputElement).value));
}

function blurTarget(e: Event) {
  (e.target as HTMLElement).blur();
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return h > 0
    ? `${h}:${mm}:${String(s).padStart(2, "0")}`
    : `${mm}:${String(s).padStart(2, "0")}`;
}

// --- Keyboard bus registration --------------------------------------------

const handle: VideoPlayerHandle = {
  togglePlay,
  pause: () => video.value?.pause(),
  seekBy,
  stepFrame,
  toggleMute,
  toggleFullscreen,
  isPlaying: () => isPlaying.value,
};

onMounted(() => {
  restoreVolume();
  const v = video.value;
  if (v) {
    v.volume = volume.value;
    v.muted = muted.value;
    // The user navigated to this clip deliberately, so start it.
    v.play().catch(() => {});
  }
  document.addEventListener("fullscreenchange", onFullscreenChange);
  registerVideoPlayer(handle);
});

onBeforeUnmount(() => {
  unregisterVideoPlayer(handle);
  document.removeEventListener("fullscreenchange", onFullscreenChange);
  if (hideHandle !== undefined) clearTimeout(hideHandle);
  if (flashHandle !== undefined) clearTimeout(flashHandle);
  // Stop audio before teardown — a fast arrow-key run through the gallery can
  // otherwise leave a sound tail bleeding over the next item.
  video.value?.pause();
});
</script>

<template>
  <div
    class="video-player"
    ref="wrapper"
    :class="{ idle: !controlsVisible, fullscreen: isFullscreen }"
    @pointermove="revealControls"
    @pointerleave="onWrapperLeave"
  >
    <video
      ref="video"
      class="video-el"
      :src="props.src"
      :poster="props.poster"
      :controls="props.nativeControls"
      :loop="loop"
      preload="metadata"
      playsinline
      @click="togglePlay"
      @loadedmetadata="syncDuration"
      @durationchange="syncDuration"
      @timeupdate="onTimeUpdate"
      @progress="updateBuffered"
      @play="onPlay"
      @pause="onPause"
      @ended="isPlaying = false"
    />

    <!-- Centre confirmation of a play/pause toggle -->
    <Transition name="flash">
      <div v-if="flash" class="flash-badge" :key="flash">
        <svg v-if="flash === 'play'" width="26" height="26" viewBox="0 0 24 24" fill="currentColor">
          <path d="M8 5v14l11-7z" />
        </svg>
        <svg v-else width="26" height="26" viewBox="0 0 24 24" fill="currentColor">
          <path d="M7 5h3.5v14H7zM13.5 5H17v14h-3.5z" />
        </svg>
      </div>
    </Transition>

    <div
      v-if="!props.nativeControls"
      class="controls"
      @mousedown="onControlMouseDown"
    >
      <div
        class="seek"
        ref="seekBar"
        @pointerdown="onSeekPointerDown"
        @pointermove="onSeekPointerMove"
        @pointerup="onSeekPointerUp"
        @pointercancel="onSeekPointerUp"
        @pointerleave="hoverFraction = null"
      >
        <div class="seek-track">
          <div
            v-for="(range, i) in bufferedRanges"
            :key="i"
            class="seek-buffered"
            :style="{ left: range.left + '%', width: range.width + '%' }"
          />
          <div class="seek-played" :style="{ width: playedPercent + '%' }" />
          <div class="seek-thumb" :style="{ left: playedPercent + '%' }" />
        </div>
        <div
          v-if="hoverFraction !== null && duration > 0"
          class="seek-tooltip"
          :style="{ left: hoverFraction * 100 + '%' }"
        >
          {{ formatTime(hoverFraction * duration) }}
        </div>
      </div>

      <div class="control-row">
        <button
          class="ctl-btn"
          @click="togglePlay"
          :title="isPlaying ? 'Pause (Space)' : 'Play (Space)'"
        >
          <svg v-if="isPlaying" width="15" height="15" viewBox="0 0 24 24" fill="currentColor">
            <path d="M7 5h3.5v14H7zM13.5 5H17v14h-3.5z" />
          </svg>
          <svg v-else width="15" height="15" viewBox="0 0 24 24" fill="currentColor">
            <path d="M8 5v14l11-7z" />
          </svg>
        </button>

        <span class="time">
          {{ formatTime(currentTime) }}
          <span class="time-sep">/</span>
          {{ formatTime(duration) }}
        </span>

        <div class="ctl-spacer"></div>

        <div class="volume">
          <button
            class="ctl-btn"
            @click="toggleMute"
            :title="muted ? 'Unmute (M)' : 'Mute (M)'"
          >
            <svg v-if="muted || volume === 0" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M11 5 6 9H3v6h3l5 4z" />
              <line x1="17" y1="9" x2="22" y2="14" />
              <line x1="22" y1="9" x2="17" y2="14" />
            </svg>
            <svg v-else width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M11 5 6 9H3v6h3l5 4z" />
              <path d="M15.5 8.5a5 5 0 0 1 0 7" />
              <path v-if="volume > 0.6" d="M18.5 6a8 8 0 0 1 0 12" />
            </svg>
          </button>
          <input
            class="vol-slider"
            type="range"
            min="0"
            max="1"
            step="0.01"
            :value="muted ? 0 : volume"
            :style="{ '--fill': (muted ? 0 : volume) * 100 + '%' }"
            @input="onVolumeInput"
            @pointerup="blurTarget"
            title="Volume"
          />
        </div>

        <div class="rate">
          <button
            class="ctl-btn ctl-text"
            :class="{ active: rate !== 1 }"
            @click="rateMenuOpen = !rateMenuOpen"
            title="Playback speed"
          >
            {{ rate }}&times;
          </button>
          <div v-if="rateMenuOpen" class="rate-menu">
            <button
              v-for="r in RATES"
              :key="r"
              :class="['rate-option', { active: r === rate }]"
              @click="setRate(r)"
            >
              {{ r }}&times;
            </button>
          </div>
        </div>

        <button
          class="ctl-btn"
          :class="{ active: loop }"
          @click="toggleLoop"
          title="Loop"
        >
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M17 2l4 4-4 4" />
            <path d="M3 11V9a4 4 0 0 1 4-4h14" />
            <path d="M7 22l-4-4 4-4" />
            <path d="M21 13v2a4 4 0 0 1-4 4H3" />
          </svg>
        </button>

        <button class="ctl-btn" @click="toggleFullscreen" title="Fullscreen (F)">
          <svg v-if="isFullscreen" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M9 3H5a2 2 0 0 0-2 2v4" />
            <path d="M15 3h4a2 2 0 0 1 2 2v4" />
            <path d="M15 21h4a2 2 0 0 0 2-2v-4" />
            <path d="M9 21H5a2 2 0 0 1-2-2v-4" />
          </svg>
          <svg v-else width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 9V5a2 2 0 0 1 2-2h4" />
            <path d="M21 9V5a2 2 0 0 0-2-2h-4" />
            <path d="M21 15v4a2 2 0 0 1-2 2h-4" />
            <path d="M3 15v4a2 2 0 0 0 2 2h4" />
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.video-player {
  position: relative;
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #050504;
}

/* Playing with no pointer movement: bar fades out and the cursor follows it,
   so the frame is unobstructed while judging a clip. */
.video-player.idle {
  cursor: none;
}

.video-el {
  max-width: 100%;
  max-height: 100%;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

/* Centre play/pause flash */
.flash-badge {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  width: 58px;
  height: 58px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(13, 12, 10, 0.72);
  backdrop-filter: blur(8px);
  border: 1px solid rgba(255, 255, 255, 0.06);
  color: var(--color-text);
  pointer-events: none;
}

.flash-enter-active,
.flash-leave-active {
  transition: all 0.35s var(--ease-out);
}

.flash-enter-from {
  opacity: 0;
  transform: translate(-50%, -50%) scale(0.85);
}

.flash-leave-to {
  opacity: 0;
  transform: translate(-50%, -50%) scale(1.15);
}

/* Control bar — same blurred-glass treatment as .nav-btn in the viewers */
.controls {
  position: absolute;
  left: 12px;
  right: 12px;
  bottom: 12px;
  z-index: 10;
  padding: 6px 10px 8px;
  border-radius: var(--radius-md);
  background: rgba(13, 12, 10, 0.72);
  backdrop-filter: blur(10px);
  border: 1px solid rgba(255, 255, 255, 0.06);
  transition: opacity var(--transition-medium), transform var(--transition-medium);
}

.video-player.idle .controls {
  opacity: 0;
  transform: translateY(6px);
  pointer-events: none;
}

/* Seek bar */
.seek {
  position: relative;
  padding: 7px 0;
  cursor: pointer;
  touch-action: none;
}

.seek-track {
  position: relative;
  height: 3px;
  border-radius: 2px;
  background: rgba(255, 255, 255, 0.12);
  transition: height var(--transition-fast);
}

.seek:hover .seek-track {
  height: 5px;
}

.seek-buffered {
  position: absolute;
  top: 0;
  bottom: 0;
  background: rgba(255, 255, 255, 0.16);
}

.seek-played {
  position: absolute;
  top: 0;
  bottom: 0;
  left: 0;
  background: var(--color-accent);
  border-radius: 2px;
}

.seek-thumb {
  position: absolute;
  top: 50%;
  width: 10px;
  height: 10px;
  margin-left: -5px;
  border-radius: 50%;
  background: var(--color-accent);
  transform: translateY(-50%) scale(0);
  transition: transform var(--transition-fast);
  box-shadow: 0 0 6px rgba(196, 162, 78, 0.5);
}

.seek:hover .seek-thumb {
  transform: translateY(-50%) scale(1);
}

.seek-tooltip {
  position: absolute;
  bottom: 22px;
  transform: translateX(-50%);
  padding: 2px 6px;
  border-radius: 4px;
  background: var(--color-surface-elevated);
  border: 1px solid var(--color-border);
  color: var(--color-text-secondary);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  pointer-events: none;
}

/* Button row */
.control-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.ctl-spacer {
  flex: 1;
}

.ctl-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  background: none;
  border: none;
  border-radius: 5px;
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: all var(--transition-fast);
}

.ctl-btn:hover {
  background: rgba(255, 255, 255, 0.07);
  color: var(--color-text);
}

.ctl-btn.active {
  color: var(--color-accent);
}

.ctl-text {
  width: auto;
  padding: 0 7px;
  font-family: var(--font-body);
  font-size: 11px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.time {
  font-size: 11px;
  color: var(--color-text-secondary);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  margin-left: 2px;
}

.time-sep {
  color: var(--color-text-muted);
  margin: 0 1px;
}

/* Volume */
.volume {
  display: flex;
  align-items: center;
  gap: 4px;
}

.vol-slider {
  -webkit-appearance: none;
  appearance: none;
  width: 62px;
  height: 3px;
  border-radius: 2px;
  background: linear-gradient(
    to right,
    var(--color-text-secondary) var(--fill),
    rgba(255, 255, 255, 0.12) var(--fill)
  );
  cursor: pointer;
  outline: none;
}

.vol-slider::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: var(--color-text);
  border: none;
}

/* Playback rate menu */
.rate {
  position: relative;
}

.rate-menu {
  position: absolute;
  bottom: calc(100% + 8px);
  right: 0;
  display: flex;
  flex-direction: column;
  padding: 3px;
  gap: 1px;
  border-radius: var(--radius-sm);
  background: var(--color-surface-elevated);
  border: 1px solid var(--color-border);
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);
}

.rate-option {
  background: none;
  border: none;
  border-radius: 4px;
  padding: 4px 12px;
  font-family: var(--font-body);
  font-size: 11px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: var(--color-text-secondary);
  cursor: pointer;
  white-space: nowrap;
  transition: all var(--transition-fast);
}

.rate-option:hover {
  background: var(--color-surface-hover);
  color: var(--color-text);
}

.rate-option.active {
  color: var(--color-accent);
}
</style>
