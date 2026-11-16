import type { ThumbnailBatch, ThumbnailResult } from "@/types";

export interface ThumbnailSink {
  /** id → thumbnail path. Written with in-place `.set` only (see stores). */
  paths: Map<string, string>;
  /** ids whose thumbnail could not be generated. */
  failed?: Set<string>;
  onProgress?: (completed: number, total: number) => void;
}

/**
 * Run a thumbnail command and fill `sink` from it.
 *
 * Channel batches are applied as they arrive, for progressive display only:
 * Tauri delivers large channel payloads through a separate async fetch, so a
 * batch can still be in flight when the command resolves. The command's
 * return value is the complete outcome and is applied before this resolves,
 * so once it resolves `sink.paths` holds every generated thumbnail (callers
 * like the CLIP indexer rely on that). Late batches only repeat entries that
 * are already present. If the command rejects, this rejects too.
 */
export async function fillThumbnails(
  run: (onBatch: (batch: ThumbnailBatch) => void) => Promise<ThumbnailResult>,
  sink: ThumbnailSink
): Promise<void> {
  let done = false;
  let completed = 0;
  const apply = (items: ThumbnailResult["items"], failed: string[]) => {
    for (const item of items) {
      // Skip unchanged entries so a repeat does not re-trigger the card.
      if (sink.paths.get(item.image_id) !== item.thumbnail_path) {
        sink.paths.set(item.image_id, item.thumbnail_path);
      }
    }
    for (const id of failed) {
      if (!sink.paths.has(id)) sink.failed?.add(id);
    }
  };

  const result = await run((batch) => {
    apply(batch.items, batch.failed);
    if (!done && batch.completed > completed) {
      completed = batch.completed;
      sink.onProgress?.(batch.completed, batch.total);
    }
  });
  done = true;
  apply(result.items, result.failed);
  const total = result.items.length + result.failed.length;
  sink.onProgress?.(total, total);
}
