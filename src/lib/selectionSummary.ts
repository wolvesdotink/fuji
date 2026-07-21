import type { ImagePair } from "@/types";

export interface SelectionSummary {
  /** Number of images in the gallery. */
  total: number;
  /** Images with no rating or an explicit 0 rating (won't be imported). */
  skip: number;
  /** Images rated 1-3 → import HEIF only. */
  heifOnly: number;
  /** Images rated 4-5 → import HEIF + RAF. */
  heifAndRaw: number;
  /** Videos with a non-zero rating → import the original movie. */
  videos: number;
  /** Images the user has acted on: total - remaining. */
  reviewed: number;
  /** Images with no rating entry at all (truly untouched). */
  remaining: number;
  /** Images that will be imported: heifOnly + heifAndRaw. */
  toImport: number;
  /** Total bytes that will be copied for the current selection. */
  bytes: number;
}

/**
 * Derive every selection-related stat the UI needs in a single O(n) pass over
 * the images. This replaces four independent computeds (`unreviewed`,
 * `selectedForImport`, `selectionSummary`, `totalImportSize`) that each walked
 * the whole gallery and re-ran on every rating change.
 *
 * Semantics preserved exactly from the original computeds:
 *  - `skip`      counts images with no rating OR an explicit 0 rating.
 *  - `remaining` counts images with no rating entry at all — distinct from
 *                `skip` because a stored rating of 0 (e.g. read from a file)
 *                counts as reviewed-but-skipped, not untouched.
 *  - `reviewed`  is `total - remaining` (a stored 0 counts as reviewed).
 *  - `bytes`     sums HIF size for every rated image, plus RAF size at 4-5.
 */
export function deriveSelectionSummary(
  images: readonly ImagePair[],
  ratings: ReadonlyMap<string, number>
): SelectionSummary {
  let skip = 0;
  let heifOnly = 0;
  let heifAndRaw = 0;
  let videos = 0;
  let remaining = 0;
  let bytes = 0;

  for (const img of images) {
    if (!ratings.has(img.id)) remaining++;

    const rating = ratings.get(img.id);
    if (!rating || rating === 0) {
      skip++;
      continue;
    }

    if (img.media_type === "Video") {
      videos++;
    } else if (rating <= 3) {
      heifOnly++;
    } else {
      heifAndRaw++;
    }

    bytes += img.hif_size;
    if (rating >= 4 && img.raf_size) {
      bytes += img.raf_size;
    }
  }

  const total = images.length;
  return {
    total,
    skip,
    heifOnly,
    heifAndRaw,
    videos,
    reviewed: total - remaining,
    remaining,
    toImport: heifOnly + heifAndRaw + videos,
    bytes,
  };
}
