/**
 * useUpdater — Tauri auto-update state machine for Vue.
 *
 * Lifecycle:
 *   idle → checking → available → downloading → ready
 *                  ↘ idle (no update)        ↘ error (any failure)
 *
 * State and side-effects are hoisted to module scope so the composable acts
 * as a singleton: any number of components can call `useUpdater()` and they
 * all share the same refs. The 4-second silent boot check fires once per app
 * launch, no matter how many times consumers mount and unmount.
 *
 * Dev-mode behavior:
 *   The updater plugin needs a signed bundle to do anything. In `pnpm tauri
 *   dev` and in the browser-only Vite build, every call throws. We catch
 *   those throws, log once, and stay in `idle` — there's nothing useful the
 *   user can do about it.
 *
 * The updater endpoint, public key, and version checks are all configured
 * in src-tauri/tauri.conf.json under `plugins.updater`. This composable does
 * NOT know the URL — that's baked into the binary at build time.
 */
import { ref, onMounted } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type UpdaterStatus =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "ready"
  | "error";

/** Wait this long after first mount before the silent check (ms). */
const BOOT_QUIET_MS = 4000;

// Module-level singleton state. All consumers of useUpdater() see the same
// refs, and the boot check / menu listener are installed exactly once for
// the lifetime of the app.
const status = ref<UpdaterStatus>("idle");
const newVersion = ref<string | null>(null);
const notes = ref<string | null>(null);
const downloaded = ref(0);
const totalBytes = ref(0);
const error = ref<string | null>(null);
const dismissed = ref(false);

/** Holds the Update handle returned by `check()` so install can use it. */
let updateHandle: Awaited<
  ReturnType<typeof import("@tauri-apps/plugin-updater").check>
> | null = null;

let bootScheduled = false;
let menuListenerInstalled = false;
let unlistenMenu: UnlistenFn | null = null;

async function checkNow(): Promise<void> {
  dismissed.value = false;
  status.value = "checking";
  error.value = null;
  try {
    // Lazy import keeps the module out of the dev/browser bundle path
    // on first paint and lets the catch below cover the "plugin not
    // present" case cleanly.
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (!update) {
      updateHandle = null;
      status.value = "idle";
      newVersion.value = null;
      notes.value = null;
      downloaded.value = 0;
      totalBytes.value = 0;
      return;
    }
    updateHandle = update;
    status.value = "available";
    newVersion.value = update.version ?? null;
    notes.value = update.body ?? null;
    downloaded.value = 0;
    totalBytes.value = 0;
  } catch (e) {
    // Most common reason in production: no network. Most common in dev:
    // plugin not active. We surface error in state for diagnostics, but
    // the topbar button stays hidden — the next boot check will retry.
    updateHandle = null;
    status.value = "error";
    error.value = (e as Error).message ?? String(e);
  }
}

async function install(): Promise<void> {
  const update = updateHandle;
  if (!update) return;
  status.value = "downloading";
  downloaded.value = 0;
  totalBytes.value = 0;
  try {
    await update.downloadAndInstall((event) => {
      // The plugin emits one of three event shapes per the Tauri 2 docs:
      //   { event: "Started",  data: { contentLength } }
      //   { event: "Progress", data: { chunkLength    } }
      //   { event: "Finished" }
      if (event.event === "Started") {
        totalBytes.value =
          (event.data as { contentLength?: number }).contentLength ?? 0;
      } else if (event.event === "Progress") {
        const chunk =
          (event.data as { chunkLength?: number }).chunkLength ?? 0;
        downloaded.value += chunk;
      } else if (event.event === "Finished") {
        status.value = "ready";
      }
    });
  } catch (e) {
    status.value = "error";
    error.value = (e as Error).message ?? String(e);
  }
}

async function restart(): Promise<void> {
  try {
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  } catch (e) {
    status.value = "error";
    error.value = (e as Error).message ?? String(e);
  }
}

function dismiss(): void {
  dismissed.value = true;
}

function scheduleBootCheck(): void {
  if (bootScheduled) return;
  bootScheduled = true;
  window.setTimeout(() => {
    void checkNow();
  }, BOOT_QUIET_MS);
}

async function ensureMenuListener(): Promise<void> {
  if (menuListenerInstalled) return;
  menuListenerInstalled = true;
  try {
    unlistenMenu = await listen("menu://check-for-updates", () => {
      void checkNow();
    });
  } catch {
    // listen() can fail in browser-only Vite mode (no Tauri host). Reset
    // the flag so a later mount in a real Tauri context can retry.
    menuListenerInstalled = false;
  }
}

export function useUpdater() {
  onMounted(() => {
    scheduleBootCheck();
    void ensureMenuListener();
  });

  return {
    status,
    newVersion,
    notes,
    downloaded,
    totalBytes,
    error,
    dismissed,
    checkNow,
    install,
    restart,
    dismiss,
  };
}

// Cleanup hook for HMR teardown. Not strictly required for production, but
// avoids leaking the menu listener across hot reloads in dev.
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    unlistenMenu?.();
    unlistenMenu = null;
    menuListenerInstalled = false;
  });
}
