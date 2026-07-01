use std::sync::{Mutex, OnceLock};

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use tauri::ipc::Channel;
use tokenizers::Tokenizer;

use super::index::SearchIndex;
use super::{image, model, text};
use crate::models::{IndexProgress, SearchResult};

/// Minimum cosine similarity for a result to be considered relevant.
const MIN_SCORE: f32 = 0.15;

/// Long-lived CLIP runtime shared across command invocations.
///
/// The previous implementation rebuilt the ONNX sessions, reloaded the
/// tokenizer, and re-read the index file on *every* search — hundreds of
/// milliseconds of setup for a few milliseconds of actual work. `ClipEngine`
/// keeps those artifacts resident: sessions and the search index are lazily
/// built on first use and reused thereafter.
///
/// Sessions are keyed by `model_dir` so that pointing at a different model
/// directory (a different library cache, or models re-downloaded elsewhere)
/// transparently rebuilds them. Each artifact lives behind its own `Mutex`; the
/// lock is held across inference, which is safe because callers already run on
/// `spawn_blocking` threads.
#[derive(Default)]
pub struct ClipEngine {
    text: Mutex<Option<(String, Session)>>,
    vision: Mutex<Option<(String, Session)>>,
    tokenizer: OnceLock<Tokenizer>,
    index: Mutex<Option<(String, SearchIndex)>>,
}

impl ClipEngine {
    /// Search the library for `query`, returning matches sorted by relevance.
    pub fn search(
        &self,
        query: String,
        model_dir: String,
        index_path: String,
    ) -> Result<Vec<SearchResult>, String> {
        log::info!("Searching for: \"{}\"", query);

        // Text embedding via the cached text session + tokenizer.
        let tok_path = model::tokenizer_path(&model_dir);
        let query_embedding = {
            let tokenizer = self.tokenizer(&tok_path)?;
            self.with_text(&model_dir, |session| {
                text::embed_text(session, tokenizer, &query)
            })?
        };

        // Brute-force search over the cached index. Loaded from disk on first
        // use or whenever the index path changes; brute force is more than fast
        // enough at this scale (a few thousand 512-dim vectors).
        let mut guard = self
            .index
            .lock()
            .map_err(|_| "CLIP index lock poisoned".to_string())?;
        let stale = !matches!(guard.as_ref(), Some((key, _)) if key == &index_path);
        if stale {
            let loaded = SearchIndex::load(&index_path)?;
            *guard = Some((index_path.clone(), loaded));
        }
        let index = &guard.as_ref().expect("index initialized above").1;
        log::info!("Loaded index with {} entries", index.entries.len());
        if index.entries.is_empty() {
            return Ok(Vec::new());
        }

        let results = index.search(&query_embedding, MIN_SCORE);
        log::info!("Search returned {} results", results.len());

        Ok(results
            .into_iter()
            .map(|(image_id, score)| SearchResult { image_id, score })
            .collect())
    }

    /// Incrementally build the search index from library thumbnails, embedding
    /// only images not already present.
    pub fn index(
        &self,
        image_ids: Vec<String>,
        thumb_paths: Vec<String>,
        model_dir: String,
        index_path: String,
        on_progress: Channel<IndexProgress>,
    ) -> Result<(), String> {
        // Load the existing index from disk for incremental updates.
        let mut index = SearchIndex::load(&index_path)?;
        let already_indexed = index.indexed_ids();

        // Filter to only new, thumbnailed images.
        let new_items: Vec<(String, String)> = image_ids
            .into_iter()
            .zip(thumb_paths)
            .filter(|(id, path)| !already_indexed.contains_key(id) && !path.is_empty())
            .collect();

        if new_items.is_empty() {
            let _ = on_progress.send(IndexProgress {
                completed: 0,
                total: 0,
            });
            self.store_index(&index_path, index);
            return Ok(());
        }

        let total = new_items.len() as u32;

        // Hold the vision session lock across the whole batch: one model load,
        // many inferences. (Already on a spawn_blocking thread.)
        self.with_vision(&model_dir, |session| {
            for (i, (image_id, thumb_path)) in new_items.iter().enumerate() {
                match image::embed_image(session, thumb_path) {
                    Ok(embedding) => index.add(image_id.clone(), embedding),
                    Err(e) => log::error!("Failed to embed image {}: {}", image_id, e),
                }

                let _ = on_progress.send(IndexProgress {
                    completed: (i + 1) as u32,
                    total,
                });
            }
            Ok(())
        })?;

        index.save(&index_path)?;
        log::info!("Search index updated: {} total entries", index.entries.len());

        // Refresh the in-memory cache so the next search sees the new
        // embeddings without re-reading the file.
        self.store_index(&index_path, index);
        Ok(())
    }

    /// Lazily load and cache the tokenizer. Loaded from the first `model_dir`
    /// seen; the tokenizer is identical across CLIP model re-downloads.
    fn tokenizer(&self, tok_path: &str) -> Result<&Tokenizer, String> {
        if let Some(tok) = self.tokenizer.get() {
            return Ok(tok);
        }
        let tok = Tokenizer::from_file(tok_path)
            .map_err(|e| format!("Failed to load tokenizer: {}", e))?;
        // Ignore the result: if another thread set it first, its value wins and
        // ours is dropped — either way the cell is populated below.
        let _ = self.tokenizer.set(tok);
        Ok(self.tokenizer.get().expect("tokenizer set above"))
    }

    fn with_text<T>(
        &self,
        model_dir: &str,
        f: impl FnOnce(&mut Session) -> Result<T, String>,
    ) -> Result<T, String> {
        run_session(&self.text, model_dir, &model::text_model_path(model_dir), f)
    }

    fn with_vision<T>(
        &self,
        model_dir: &str,
        f: impl FnOnce(&mut Session) -> Result<T, String>,
    ) -> Result<T, String> {
        run_session(
            &self.vision,
            model_dir,
            &model::vision_model_path(model_dir),
            f,
        )
    }

    fn store_index(&self, index_path: &str, index: SearchIndex) {
        if let Ok(mut guard) = self.index.lock() {
            *guard = Some((index_path.to_string(), index));
        }
    }
}

/// Ensure `slot` holds a session for `model_dir` (rebuilding on a key change),
/// then run `f` against it while holding the lock.
fn run_session<T>(
    slot: &Mutex<Option<(String, Session)>>,
    model_dir: &str,
    model_path: &str,
    f: impl FnOnce(&mut Session) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = slot
        .lock()
        .map_err(|_| "CLIP session lock poisoned".to_string())?;

    let stale = !matches!(guard.as_ref(), Some((key, _)) if key == model_dir);
    if stale {
        let session = build_session(model_path)?;
        *guard = Some((model_dir.to_string(), session));
    }

    let session = &mut guard.as_mut().expect("session initialized above").1;
    f(session)
}

fn build_session(model_path: &str) -> Result<Session, String> {
    let mut builder = Session::builder()
        .map_err(|e| format!("Failed to create session builder: {}", e))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("Failed to set optimization level: {}", e))?;

    // Off-by-default CoreML hook. The quantized CLIP models regress badly on the
    // CoreML EP, so this is compiled in only under `--features coreml`.
    #[cfg(feature = "coreml")]
    {
        builder = builder
            .with_execution_providers([
                ort::execution_providers::CoreMLExecutionProvider::default().build(),
            ])
            .map_err(|e| format!("Failed to register CoreML execution provider: {}", e))?;
    }

    builder
        .commit_from_file(model_path)
        .map_err(|e| format!("Failed to load model {}: {}", model_path, e))
}
