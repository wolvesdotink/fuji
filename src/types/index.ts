export type CameraSourceType = "MassStorage" | "Ptp";

export interface CameraVolume {
  name: string;
  mount_path: string;
  dcim_path: string;
  source_type: CameraSourceType;
}

export interface ImagePair {
  id: string;
  hif_path: string;
  raf_path: string | null;
  hif_size: number;
  raf_size: number | null;
}

export type StarRating = 0 | 1 | 2 | 3 | 4 | 5;

export type SelectionChoice = "Skip" | "HeifOnly" | "HeifAndRaw";

export interface ImportSelection {
  image_id: string;
  choice: SelectionChoice;
  hif_path: string;
  raf_path: string | null;
  rating: number | null;
}

export interface ImportProgress {
  current_file: string;
  files_completed: number;
  files_total: number;
  bytes_copied: number;
  bytes_total: number;
  phase: ImportPhase;
}

export type ImportPhase =
  | "CopyingToLaCie"
  | "ImportingToPhotos"
  | "Verifying"
  | "Complete";

export interface ThumbnailProgress {
  image_id: string;
  thumbnail_path: string;
  completed: number;
  total: number;
}

// --- App Config ---

export interface AppConfig {
  destination_path: string | null;
}

// --- Library ---

export interface LibraryImage {
  id: string;
  file_path: string;
  file_name: string;
  file_size: number;
  date_created: number;
  date_modified: number;
}

// --- CLIP Search ---

export interface SearchResult {
  image_id: string;
  score: number;
}

export interface ModelDownloadProgress {
  bytes_downloaded: number;
  bytes_total: number;
  file_name: string;
}

export interface IndexProgress {
  completed: number;
  total: number;
}
