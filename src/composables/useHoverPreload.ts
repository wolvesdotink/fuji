import { fileUrl } from "@/lib/commands";

// Module-level decoded-image LRU — persists across component instances for
// the session. Holding a reference to a *decoded* <img> keeps its bitmap
// warm, so the viewer can swap to full-res without a synchronous decode
// stall on the navigation path. Bounded because full-res bitmaps are large
// (tens of MB each); cap ~7 keeps a handful of neighbors hot without
// letting memory grow unbounded as the user walks a 5k gallery.
const DECODE_LRU_CAP = 7;
const decodedLru = new Map<string, HTMLImageElement>();

// Dedup in-flight decodes so rapid navigation / re-hover doesn't kick off
// duplicate decode work (or duplicate network fetches) for the same url.
const decodeInFlight = new Map<string, Promise<void>>();

function retain(url: string, img: HTMLImageElement) {
  // Refresh recency: delete + re-insert moves the key to the end of the
  // Map's insertion order, so the oldest key is always first.
  decodedLru.delete(url);
  decodedLru.set(url, img);
  while (decodedLru.size > DECODE_LRU_CAP) {
    const oldest = decodedLru.keys().next().value as string | undefined;
    if (oldest === undefined) break;
    decodedLru.delete(oldest);
  }
}

/**
 * Decode `url` off the current task and retain the decoded element in a
 * bounded LRU. Best-effort and idempotent — completed decodes (in the LRU)
 * and in-flight decodes are deduped, so this is safe to call on every
 * navigation for every neighbor.
 */
export function decodeAhead(url: string): void {
  if (!url) return;
  const warm = decodedLru.get(url);
  if (warm) {
    retain(url, warm); // Already decoded — just bump recency.
    return;
  }
  if (decodeInFlight.has(url)) return;

  const img = new Image();
  img.src = url;
  const p = img
    .decode()
    .then(() => {
      retain(url, img);
    })
    .catch(() => {
      // decode() rejects if the element is detached or the src fails to
      // load. This is a prefetch — swallow and let the real <img> surface
      // any genuine error.
    })
    .finally(() => {
      decodeInFlight.delete(url);
    });
  decodeInFlight.set(url, p);
}

export function useHoverPreload() {
  let timer: number | null = null;

  function startPreload(filePath: string) {
    const url = fileUrl(filePath);
    if (decodedLru.has(url) || decodeInFlight.has(url)) return;
    cancelPreload();
    timer = window.setTimeout(() => {
      decodeAhead(url);
    }, 300);
  }

  function cancelPreload() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  return { startPreload, cancelPreload };
}
