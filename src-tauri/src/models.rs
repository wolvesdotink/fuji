use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CameraSourceType {
    MassStorage,
    Ptp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraVolume {
    pub name: String,
    pub mount_path: String,
    pub dcim_path: String,
    pub source_type: CameraSourceType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePair {
    pub id: String,
    pub hif_path: String,
    pub raf_path: Option<String>,
    pub hif_size: u64,
    pub raf_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SelectionChoice {
    Skip,
    HeifOnly,
    HeifAndRaw,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSelection {
    pub image_id: String,
    pub choice: SelectionChoice,
    pub hif_path: String,
    pub raf_path: Option<String>,
    pub rating: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportProgress {
    pub current_file: String,
    pub files_completed: u32,
    pub files_total: u32,
    pub bytes_copied: u64,
    pub bytes_total: u64,
    pub phase: ImportPhase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImportPhase {
    CopyingToLaCie,
    ImportingToPhotos,
    Verifying,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThumbnailProgress {
    pub image_id: String,
    pub thumbnail_path: String,
    pub completed: u32,
    pub total: u32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub file_path: String,
    pub source_size: u64,
    pub dest_size: u64,
    pub matches: bool,
}

// --- App Config ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub destination_path: Option<String>,
}

// --- Library ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryImage {
    pub id: String,
    pub file_path: String,
    pub file_name: String,
    pub file_size: u64,
    pub date_created: u64,
    pub date_modified: u64,
}

// --- CLIP Search ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub image_id: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDownloadProgress {
    pub bytes_downloaded: u64,
    pub bytes_total: u64,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexProgress {
    pub completed: u32,
    pub total: u32,
}

// --- Persistent Image Index ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexFingerprint {
    pub file_count: u64,
    pub newest_mtime: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageIndex<T> {
    pub version: u32,
    pub source_path: String,
    pub fingerprint: IndexFingerprint,
    pub images: Vec<T>,
}
