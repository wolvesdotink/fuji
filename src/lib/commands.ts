import { invoke, convertFileSrc, Channel } from "@tauri-apps/api/core";
import type {
  CameraVolume,
  ImagePair,
  ImportSelection,
  ImportProgress,
  ThumbnailProgress,
  AppConfig,
  LibraryImage,
  SearchResult,
  ModelDownloadProgress,
  IndexProgress,
} from "@/types";

export async function scanForCameras(): Promise<CameraVolume[]> {
  return invoke("scan_for_cameras");
}

export interface CameraDiagnostics {
  timestamp: string;
  ptp: {
    binary_path: string;
    binary_exists: boolean;
    scan_stdout: string;
    scan_stderr: string;
    invocation_error: string | null;
  };
  codesign: string;
  volumes: Array<{ name: string; path: string; has_dcim: boolean }>;
}

export async function cameraDiagnostics(): Promise<CameraDiagnostics> {
  return invoke("camera_diagnostics");
}

export async function listImages(dcimPath: string, cacheDir: string): Promise<ImagePair[]> {
  return invoke("list_images", { dcimPath, cacheDir });
}

export async function ptpListImages(
  cameraName: string,
  thumbCacheDir: string
): Promise<ImagePair[]> {
  return invoke("ptp_list_images", { cameraName, thumbCacheDir });
}

export async function ptpDownloadFile(
  cameraName: string,
  fileName: string,
  destDir: string
): Promise<string> {
  return invoke("ptp_download_file", { cameraName, fileName, destDir });
}

export async function ptpDeleteFiles(
  cameraName: string,
  fileNames: string[]
): Promise<number> {
  return invoke("ptp_delete_files", { cameraName, fileNames });
}

export async function ptpImportFiles(
  cameraName: string,
  selections: ImportSelection[],
  destDir: string,
  onProgress: (progress: ImportProgress) => void
): Promise<void> {
  const channel = new Channel<ImportProgress>();
  channel.onmessage = onProgress;
  return invoke("ptp_import_files", {
    cameraName,
    selections,
    destDir,
    onProgress: channel,
  });
}

export async function generateThumbnails(
  dcimPath: string,
  imageIds: string[],
  rafPaths: string[],
  cacheDir: string,
  onProgress: (progress: ThumbnailProgress) => void
): Promise<[string, string][]> {
  const channel = new Channel<ThumbnailProgress>();
  channel.onmessage = onProgress;
  return invoke("generate_thumbnails", {
    dcimPath,
    imageIds,
    rafPaths,
    cacheDir,
    onProgress: channel,
  });
}

export async function getThumbnail(
  imageId: string,
  rafPath: string,
  cacheDir: string
): Promise<string> {
  return invoke("get_thumbnail", { imageId, rafPath, cacheDir });
}

export async function importFiles(
  selections: ImportSelection[],
  destDir: string,
  onProgress: (progress: ImportProgress) => void
): Promise<void> {
  const channel = new Channel<ImportProgress>();
  channel.onmessage = onProgress;
  return invoke("import_files", {
    selections,
    destDir,
    onProgress: channel,
  });
}

export async function getFilesToDelete(
  selections: ImportSelection[]
): Promise<string[]> {
  return invoke("get_files_to_delete", { selections });
}

export async function deleteFromCamera(
  filePaths: string[]
): Promise<number> {
  return invoke("delete_from_camera", { filePaths });
}

/** Convert a local file path to a URL the webview can load */
export function fileUrl(path: string): string {
  return convertFileSrc(path);
}

// --- Config ---

export async function loadConfig(configPath: string): Promise<AppConfig> {
  return invoke("load_config", { configPath });
}

export async function saveConfig(
  configPath: string,
  config: AppConfig
): Promise<void> {
  return invoke("save_config", { configPath, config });
}

// --- Ratings ---

export async function readFileRatings(
  filePaths: string[],
  cacheDir: string
): Promise<Record<string, number>> {
  return invoke("read_file_ratings", { filePaths, cacheDir });
}

export async function writeFileRating(
  filePath: string,
  rating: number
): Promise<void> {
  return invoke("write_file_rating", { filePath, rating });
}

// --- Library ---

export async function listLibraryImages(
  dirPath: string,
  cacheDir: string
): Promise<LibraryImage[]> {
  return invoke("list_library_images", { dirPath, cacheDir });
}

export async function generateLibraryThumbnails(
  imagePaths: string[],
  imageIds: string[],
  cacheDir: string,
  onProgress: (progress: ThumbnailProgress) => void
): Promise<[string, string][]> {
  const channel = new Channel<ThumbnailProgress>();
  channel.onmessage = onProgress;
  return invoke("generate_library_thumbnails", {
    imagePaths,
    imageIds,
    cacheDir,
    onProgress: channel,
  });
}

// --- CLIP Search ---

export async function ensureClipModels(
  cacheDir: string,
  onProgress: (progress: ModelDownloadProgress) => void
): Promise<void> {
  const channel = new Channel<ModelDownloadProgress>();
  channel.onmessage = onProgress;
  return invoke("ensure_clip_models", { cacheDir, onProgress: channel });
}

export async function indexLibrary(
  imageIds: string[],
  thumbPaths: string[],
  modelDir: string,
  indexPath: string,
  onProgress: (progress: IndexProgress) => void
): Promise<void> {
  const channel = new Channel<IndexProgress>();
  channel.onmessage = onProgress;
  return invoke("index_library", {
    imageIds,
    thumbPaths,
    modelDir,
    indexPath,
    onProgress: channel,
  });
}

export async function searchLibrary(
  query: string,
  modelDir: string,
  indexPath: string
): Promise<SearchResult[]> {
  return invoke("search_library", { query, modelDir, indexPath });
}
