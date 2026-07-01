use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

/// A search index entry: image ID mapped to its CLIP embedding.
pub struct IndexEntry {
    pub image_id: String,
    pub embedding: Vec<f32>,
}

/// In-memory search index.
pub struct SearchIndex {
    pub entries: Vec<IndexEntry>,
}

impl SearchIndex {
    pub fn new() -> Self {
        SearchIndex {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, image_id: String, embedding: Vec<f32>) {
        self.entries.push(IndexEntry {
            image_id,
            embedding,
        });
    }

    /// Get the set of already-indexed image IDs.
    pub fn indexed_ids(&self) -> HashMap<String, bool> {
        self.entries
            .iter()
            .map(|e| (e.image_id.clone(), true))
            .collect()
    }

    /// Search the index using a query embedding.
    /// Returns (image_id, cosine_similarity) pairs sorted by score descending.
    pub fn search(&self, query_embedding: &[f32], min_score: f32) -> Vec<(String, f32)> {
        let mut results: Vec<(String, f32)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let score = cosine_similarity(query_embedding, &entry.embedding);
                if score >= min_score {
                    Some((entry.image_id.clone(), score))
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// Save the index to a binary file.
    /// Format: [count: u32] then for each entry:
    ///   [id_len: u32, id_bytes, embedding_dim: u32, embedding_f32s]
    pub fn save(&self, path: &str) -> Result<(), String> {
        let dest = Path::new(path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create index directory: {}", e))?;
        }

        let file = fs::File::create(dest)
            .map_err(|e| format!("Failed to create index file: {}", e))?;
        // Coalesce the many tiny writes (per id-length, per id, per f32) into a
        // few large syscalls; the on-disk format is byte-for-byte unchanged.
        let mut writer = BufWriter::new(file);

        let count = self.entries.len() as u32;
        writer
            .write_all(&count.to_le_bytes())
            .map_err(|e| format!("Write error: {}", e))?;

        for entry in &self.entries {
            let id_bytes = entry.image_id.as_bytes();
            let id_len = id_bytes.len() as u32;
            writer
                .write_all(&id_len.to_le_bytes())
                .map_err(|e| format!("Write error: {}", e))?;
            writer
                .write_all(id_bytes)
                .map_err(|e| format!("Write error: {}", e))?;

            let dim = entry.embedding.len() as u32;
            writer
                .write_all(&dim.to_le_bytes())
                .map_err(|e| format!("Write error: {}", e))?;

            for val in &entry.embedding {
                writer
                    .write_all(&val.to_le_bytes())
                    .map_err(|e| format!("Write error: {}", e))?;
            }
        }

        // Surface any deferred write errors before we report success.
        writer
            .flush()
            .map_err(|e| format!("Write error: {}", e))?;

        Ok(())
    }

    /// Load the index from a binary file.
    pub fn load(path: &str) -> Result<Self, String> {
        let dest = Path::new(path);
        if !dest.exists() {
            return Ok(SearchIndex::new());
        }

        let file = fs::File::open(dest)
            .map_err(|e| format!("Failed to open index file: {}", e))?;
        // Read through a buffer so the per-field read_exact calls are served
        // from memory instead of hitting the filesystem thousands of times.
        let mut reader = BufReader::new(file);

        let mut buf4 = [0u8; 4];

        reader
            .read_exact(&mut buf4)
            .map_err(|e| format!("Read error: {}", e))?;
        let count = u32::from_le_bytes(buf4) as usize;

        let mut entries = Vec::with_capacity(count);

        for _ in 0..count {
            reader
                .read_exact(&mut buf4)
                .map_err(|e| format!("Read error: {}", e))?;
            let id_len = u32::from_le_bytes(buf4) as usize;

            let mut id_bytes = vec![0u8; id_len];
            reader
                .read_exact(&mut id_bytes)
                .map_err(|e| format!("Read error: {}", e))?;
            let image_id = String::from_utf8(id_bytes)
                .map_err(|e| format!("UTF-8 error: {}", e))?;

            reader
                .read_exact(&mut buf4)
                .map_err(|e| format!("Read error: {}", e))?;
            let dim = u32::from_le_bytes(buf4) as usize;

            let mut embedding = Vec::with_capacity(dim);
            for _ in 0..dim {
                reader
                    .read_exact(&mut buf4)
                    .map_err(|e| format!("Read error: {}", e))?;
                embedding.push(f32::from_le_bytes(buf4));
            }

            entries.push(IndexEntry {
                image_id,
                embedding,
            });
        }

        Ok(SearchIndex { entries })
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    // Vectors are already L2-normalized, so dot product = cosine similarity
    dot
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_index_path(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fuji_{}_{}.idx", tag, nanos))
    }

    // Save through BufWriter, load back through BufReader, and confirm every id
    // and embedding value survives the round-trip bit-for-bit. Includes a
    // zero-length id and varied embedding dims to exercise the length prefixes.
    #[test]
    fn save_load_round_trip_preserves_entries() {
        let path = temp_index_path("index_roundtrip");
        let path_str = path.to_string_lossy().to_string();

        let mut index = SearchIndex::new();
        index.add("img-001".to_string(), vec![0.0, 1.0, -0.5, 0.25]);
        index.add("".to_string(), vec![12.5, -37.125]);
        index.add("ünîcodë".to_string(), vec![1.0; 512]);

        index.save(&path_str).unwrap();
        let loaded = SearchIndex::load(&path_str).unwrap();

        assert_eq!(loaded.entries.len(), index.entries.len());
        for (orig, back) in index.entries.iter().zip(loaded.entries.iter()) {
            assert_eq!(orig.image_id, back.image_id);
            assert_eq!(orig.embedding, back.embedding);
        }

        fs::remove_file(&path).ok();
    }

    // Loading a path that was never written must yield an empty index, not an
    // error — search_library relies on this for a cold cache.
    #[test]
    fn load_missing_file_returns_empty_index() {
        let path = temp_index_path("index_missing");
        let loaded = SearchIndex::load(&path.to_string_lossy()).unwrap();
        assert!(loaded.entries.is_empty());
    }
}
