use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

use crate::models::{ImageIndex, IndexFingerprint};

const INDEX_VERSION: u32 = 1;

/// Incrementally folds matched files into the same [`IndexFingerprint`] that
/// [`compute_fingerprint`] produces. A full directory scan can accumulate this
/// alongside its own results and emit the fingerprint from a single walk,
/// avoiding a separate fingerprint pass on a cache miss / first run.
#[derive(Default)]
pub struct FingerprintAccumulator {
    file_count: u64,
    newest_mtime: u64,
    total_bytes: u64,
}

impl FingerprintAccumulator {
    /// Fold one matched file's metadata into the fingerprint. Callers must only
    /// pass files that pass the same extension filter used by
    /// `compute_fingerprint`, so the two agree byte-for-byte.
    pub fn add(&mut self, metadata: &std::fs::Metadata) {
        self.file_count += 1;
        self.total_bytes += metadata.len();

        if let Ok(modified) = metadata.modified() {
            if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                let mtime = duration.as_secs();
                if mtime > self.newest_mtime {
                    self.newest_mtime = mtime;
                }
            }
        }
    }

    pub fn finish(self) -> IndexFingerprint {
        IndexFingerprint {
            file_count: self.file_count,
            newest_mtime: self.newest_mtime,
            total_bytes: self.total_bytes,
        }
    }
}

/// Compute a lightweight fingerprint for a directory by walking it and collecting
/// only file count, max mtime, and total bytes — no string allocation for paths/stems.
pub fn compute_fingerprint(
    dir: &Path,
    extensions: &[&str],
    max_depth: Option<usize>,
) -> Result<IndexFingerprint, String> {
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }

    let mut acc = FingerprintAccumulator::default();

    let walker = if let Some(depth) = max_depth {
        WalkDir::new(dir).min_depth(1).max_depth(depth)
    } else {
        WalkDir::new(dir).min_depth(1)
    };

    for entry in walker.into_iter().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_uppercase();

        if !extensions.iter().any(|e| e.to_uppercase() == ext) {
            continue;
        }

        if let Ok(metadata) = entry.metadata() {
            acc.add(&metadata);
        }
    }

    Ok(acc.finish())
}

/// Load a cached index from a JSON file.
pub fn load_index<T: DeserializeOwned>(index_path: &Path) -> Result<ImageIndex<T>, String> {
    let data =
        fs::read_to_string(index_path).map_err(|e| format!("Failed to read index: {}", e))?;
    serde_json::from_str(&data).map_err(|e| format!("Failed to parse index: {}", e))
}

/// Save an index to a JSON file.
pub fn save_index<T: Serialize>(index_path: &Path, index: &ImageIndex<T>) -> Result<(), String> {
    let data =
        serde_json::to_string(index).map_err(|e| format!("Failed to serialize index: {}", e))?;
    fs::write(index_path, data).map_err(|e| format!("Failed to write index: {}", e))
}

/// Try to load a cached index and return it if the fingerprint matches.
/// Returns None if the cache is stale or missing.
pub fn try_cached<T: DeserializeOwned>(
    index_path: &Path,
    source_path: &str,
    fingerprint: &IndexFingerprint,
) -> Option<Vec<T>> {
    let cached = load_index::<T>(index_path).ok()?;
    if cached.version == INDEX_VERSION
        && cached.source_path == source_path
        && cached.fingerprint == *fingerprint
    {
        Some(cached.images)
    } else {
        None
    }
}

/// Save images to the index cache.
pub fn cache_images<T: Serialize + Clone>(
    index_path: &Path,
    source_path: &str,
    fingerprint: &IndexFingerprint,
    images: &[T],
) -> Result<(), String> {
    let index = ImageIndex {
        version: INDEX_VERSION,
        source_path: source_path.to_string(),
        fingerprint: fingerprint.clone(),
        images: images.to_vec(),
    };
    save_index(index_path, &index)
}
