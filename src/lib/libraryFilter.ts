import type { MediaType } from "@/types";

/**
 * Library rating filter. A number N means "N stars and up"; "unrated" keeps
 * only media without a rating.
 */
export type RatingFilter = "all" | "unrated" | 1 | 2 | 3 | 4 | 5;

/** Library media filter: everything, photos only, or videos only. */
export type MediaFilter = "all" | MediaType;

/** Rating filter options in the order the filter popover lists them. */
export const RATING_FILTERS: readonly RatingFilter[] = [
  "all",
  5,
  4,
  3,
  2,
  1,
  "unrated",
];

export const MEDIA_FILTERS: readonly { value: MediaFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "Image", label: "Photos" },
  { value: "Video", label: "Videos" },
];

export function ratingFilterLabel(f: RatingFilter): string {
  if (f === "all") return "Any rating";
  if (f === "unrated") return "Unrated";
  return f === 5 ? "5 stars" : `${f} stars and up`;
}

export function mediaFilterLabel(f: MediaFilter): string {
  return f === "Image" ? "Photos" : f === "Video" ? "Videos" : "All media";
}

/** Plain-language summary of the active filters, e.g. "3 stars and up, videos". */
export function describeFilters(rating: RatingFilter, media: MediaFilter): string {
  const parts: string[] = [];
  if (rating !== "all") parts.push(ratingFilterLabel(rating));
  if (media !== "all") parts.push(mediaFilterLabel(media).toLowerCase());
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}
