/**
 * Review-minimap aggregation.
 *
 * The header minimap used to render one `<div>` per image, each calling
 * `segmentStatus(img.id)` in the render function. For a 5k-image shoot that is
 * 5000 always-visible DOM nodes (content-visibility can't skip them — they live
 * in the header) whose render function depends on every rating key, so *every*
 * keystroke while culling re-ran 5000 status lookups and diffed 5000 nodes.
 *
 * Instead we aggregate large galleries into at most `maxSegments` buckets in a
 * single O(n) pass (`buildMinimapSegments`) and derive the current-position
 * highlight with `minimapBucketOf`, which uses the *same* bucket formula so a
 * click on a bucket and the highlight of the current image stay consistent.
 */

/** CSS class describing a segment's fill colour. */
export type MinimapSegClass = "seg-unreviewed" | "seg-heif" | "seg-heif-raw";

export interface MinimapSegment {
  /** CSS class for the segment's colour. */
  status: MinimapSegClass;
  /**
   * Image index this segment jumps to when clicked — the first image that
   * falls in the bucket, so clicking near the left of a bucket lands on its
   * start (matches the pre-aggregation one-div-per-image behaviour when not
   * aggregating).
   */
  repIndex: number;
}

export interface MinimapItem {
  id: string;
}

/**
 * Aggregate a gallery into minimap segments.
 *
 * - Below/at `aggregateAbove` images, every image gets its own segment, so the
 *   behaviour is byte-for-byte identical to the old one-div-per-image minimap
 *   (bucketCount === total, repIndex === index).
 * - Above the threshold the gallery collapses into `min(maxSegments, total)`
 *   contiguous buckets. A bucket's colour is the *highest* rating class present
 *   in it (unrated < 1-3 < 4-5) so a single kept frame in a sea of unreviewed
 *   ones still shows up.
 *
 * Reads each rating exactly once. When called inside a Vue `computed`, the
 * per-key `ratings.get(...)` reads register the usual reactive dependencies, so
 * the computed re-runs on a rating change but not on mere navigation.
 */
export function buildMinimapSegments(
  items: readonly MinimapItem[],
  ratings: ReadonlyMap<string, number>,
  opts: { maxSegments?: number; aggregateAbove?: number } = {}
): MinimapSegment[] {
  const maxSegments = opts.maxSegments ?? 200;
  const aggregateAbove = opts.aggregateAbove ?? 500;
  const total = items.length;
  if (total === 0) return [];

  const bucketCount =
    total > aggregateAbove ? Math.min(maxSegments, total) : total;

  // best: 0 = unreviewed, 1 = heif (1-3 stars), 2 = heif+raw (4-5 stars).
  const best = new Int8Array(bucketCount);
  const repIndex = new Int32Array(bucketCount);
  const seen = new Uint8Array(bucketCount);

  for (let i = 0; i < total; i++) {
    const b = Math.floor((i * bucketCount) / total);
    if (!seen[b]) {
      seen[b] = 1;
      repIndex[b] = i;
    }
    const rating = ratings.get(items[i].id);
    if (rating && rating > 0) {
      const cls = rating <= 3 ? 1 : 2;
      if (cls > best[b]) best[b] = cls;
    }
  }

  const segments: MinimapSegment[] = new Array(bucketCount);
  for (let b = 0; b < bucketCount; b++) {
    segments[b] = {
      status:
        best[b] === 0
          ? "seg-unreviewed"
          : best[b] === 1
            ? "seg-heif"
            : "seg-heif-raw",
      repIndex: repIndex[b],
    };
  }
  return segments;
}

/**
 * Which segment contains `index`, using the same bucket formula as
 * `buildMinimapSegments`. Returns -1 when the index is out of range (e.g. no
 * current image, or the minimap isn't showing a position). `bucketCount` must
 * be the length returned by `buildMinimapSegments` for the same gallery.
 */
export function minimapBucketOf(
  index: number,
  total: number,
  bucketCount: number
): number {
  if (total <= 0 || bucketCount <= 0 || index < 0 || index >= total) return -1;
  return Math.floor((index * bucketCount) / total);
}
