use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

use crate::imageio;
use crate::models::{ThumbnailBatch, ThumbnailEntry, ThumbnailResult};
use crate::raf::preview;

/// Thumbnails are at most this many pixels wide after rotation, so landscape
/// shots come out 600x400 and portrait 3:2 shots 600x900. Grid cards are 3:2
/// landscape with `object-fit: cover`, so a portrait thumbnail is cropped to its
/// width; sizing by width keeps portrait cards as sharp as landscape ones.
pub(crate) const THUMBNAIL_MAX_WIDTH: u32 = 600;

/// JPEG quality for stills (ImageIO's 0..1 scale).
const THUMBNAIL_QUALITY: f64 = 0.8;

/// Cache hits are reported in chunks of this size.
const HIT_CHUNK: usize = 1000;
/// Freshly generated thumbnails are flushed once this many are pending...
const FLUSH_ITEMS: usize = 64;
/// ...or once this much time has passed since the last flush.
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Concurrent Quick Look (`qlmanage`) runs for videos. Each one is a separate
/// process that decodes a movie, so keep it well below the still-image worker
/// count.
pub(crate) const MAX_VIDEO_JOBS: usize = 3;

pub fn is_video_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_uppercase()
            .as_str(),
        "MOV" | "MP4" | "M4V" | "AVI"
    )
}

fn is_raf_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("raf"))
        .unwrap_or(false)
}

/// Thumbnail decode slots, shared process-wide: all cores but two. Stills decode in-process
/// with ImageIO, so there is no process-per-file fan-out to bound any more
/// (videos are capped separately by [`MAX_VIDEO_JOBS`]). On a 10-core M1 Max,
/// 8 workers matched 10 in throughput; the two spare cores keep the webview
/// responsive while a large card is processed.
pub(crate) fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).max(2))
        .unwrap_or(4)
}

/// Generate a poster frame without decoding the movie in the webview. macOS
/// Quick Look understands the codecs produced by Fuji cameras; `sips` then
/// converts its PNG output to the same compact JPEG format as image thumbs.
pub fn generate_video_thumbnail(
    video_path: &str,
    thumb_path: &Path,
    max_width: u32,
) -> Result<(), String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_dir = thumb_path.with_extension(format!("qlthumb-{}-{}", std::process::id(), nonce));
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create Quick Look temp directory: {}", e))?;

    let result = (|| {
        let max_width_arg = max_width.to_string();
        let output = Command::new("qlmanage")
            .args(["-t", "-s", max_width_arg.as_str(), "-o"])
            .arg(&temp_dir)
            .arg(video_path)
            .output()
            .map_err(|e| format!("Failed to run qlmanage: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "Quick Look failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let quicklook_png = fs::read_dir(&temp_dir)
            .map_err(|e| format!("Failed to read Quick Look output: {}", e))?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .map(|ext| ext.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
            .ok_or_else(|| "Quick Look did not produce a video thumbnail".to_string())?;

        let output = Command::new("sips")
            .args(["-s", "format", "jpeg", "-s", "formatOptions", "80"])
            .arg(&quicklook_png)
            .arg("--out")
            .arg(thumb_path)
            .output()
            .map_err(|e| format!("Failed to convert video thumbnail: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "sips failed for video thumbnail: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    })();

    let _ = fs::remove_dir_all(&temp_dir);
    result
}

/// JPEG thumbnail bytes for a still: HEIF/HIF/JPEG files go straight to
/// ImageIO; for a RAF only the embedded JPEG preview is read and decoded.
pub(crate) fn still_thumbnail_jpeg(source_path: &str) -> Result<Vec<u8>, String> {
    if is_raf_path(source_path) {
        let embedded = preview::extract_jpeg_from_raf(Path::new(source_path))?;
        imageio::jpeg_thumbnail(
            imageio::Source::Bytes(&embedded),
            THUMBNAIL_MAX_WIDTH,
            THUMBNAIL_QUALITY,
        )
    } else {
        imageio::jpeg_thumbnail(
            imageio::Source::File(Path::new(source_path)),
            THUMBNAIL_MAX_WIDTH,
            THUMBNAIL_QUALITY,
        )
    }
}

/// Write the thumbnail for `source_path` to `thumb_path`. Output goes to a
/// temporary sibling first and is renamed into place, so an interrupted run
/// never leaves a truncated file that later loads would treat as cached.
pub(crate) fn generate_thumbnail_file(source_path: &str, thumb_path: &Path) -> Result<(), String> {
    // `{id}.v3.tmp-{pid}-{n}.jpg`: unique per call, never matches the cache
    // key, and keeps a `.jpg` extension for `sips --out`.
    static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
    let tmp = thumb_path.with_extension(format!(
        "tmp-{}-{}.jpg",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = if is_video_path(source_path) {
        generate_video_thumbnail(source_path, &tmp, THUMBNAIL_MAX_WIDTH)
    } else {
        still_thumbnail_jpeg(source_path).and_then(|jpeg| {
            fs::write(&tmp, &jpeg).map_err(|e| format!("Failed to write thumbnail: {}", e))
        })
    };
    let result = result.and_then(|_| {
        fs::rename(&tmp, thumb_path).map_err(|e| format!("Failed to store thumbnail: {}", e))
    });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Counting semaphore.
pub(crate) struct Semaphore {
    free: Mutex<usize>,
    cv: Condvar,
}

impl Semaphore {
    pub(crate) fn new(n: usize) -> Self {
        Semaphore {
            free: Mutex::new(n.max(1)),
            cv: Condvar::new(),
        }
    }

    fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Permit<'a>(&'a Semaphore);
        impl Drop for Permit<'_> {
            fn drop(&mut self) {
                *self.0.free.lock().unwrap() += 1;
                self.0.cv.notify_one();
            }
        }
        let mut free = self.free.lock().unwrap();
        while *free == 0 {
            free = self.cv.wait(free).unwrap();
        }
        *free -= 1;
        drop(free);
        // Released on drop, so a panicking job cannot leak its permit.
        let _permit = Permit(self);
        f()
    }
}

/// Concurrency limits for thumbnail work. The commands share one process-wide
/// pool ([`ThumbnailPool::global`]), so a library load and a camera catalog
/// running at the same time split the same decode and Quick Look slots
/// instead of doubling them.
pub(crate) struct ThumbnailPool {
    /// Threads spawned per call (each blocks on a slot before working).
    workers: usize,
    /// In-process still decodes running at once.
    decode: Semaphore,
    /// Quick Look (`qlmanage`) processes running at once.
    video: Semaphore,
}

impl ThumbnailPool {
    pub(crate) fn new(workers: usize, video_jobs: usize) -> Self {
        let workers = workers.max(1);
        ThumbnailPool {
            workers,
            decode: Semaphore::new(workers),
            video: Semaphore::new(video_jobs.min(workers)),
        }
    }

    pub(crate) fn global() -> &'static ThumbnailPool {
        static POOL: OnceLock<ThumbnailPool> = OnceLock::new();
        POOL.get_or_init(|| ThumbnailPool::new(default_workers(), MAX_VIDEO_JOBS))
    }
}

/// One thumbnail to generate. Several ids can map to the same cache file (the
/// recursive library can hold the same camera file name in several folders);
/// they share one job so the file is decoded and written once.
struct Job<'a> {
    source: &'a str,
    thumb_path: PathBuf,
    ids: Vec<&'a String>,
}

/// Generate (or find cached) thumbnails `{id}.v3.jpg` in `cache` for each
/// `(id, source)` pair. Progress goes to `send` in coalesced batches; the
/// return value is the complete outcome, which callers should treat as the
/// source of truth (IPC channel messages can arrive after the command
/// resolves).
///
/// Cached thumbnails are reported first, in chunks of [`HIT_CHUNK`]. Missing
/// ones are then generated by worker threads that pull jobs in list order, so
/// the top of the grid (the list is newest first) fills in first. Finished
/// results are delivered at most [`FLUSH_INTERVAL`] after they complete, even
/// while every worker is still busy.
pub(crate) fn generate_thumbnail_batch(
    ids: &[String],
    sources: &[String],
    cache: &Path,
    pool: &ThumbnailPool,
    send: &(dyn Fn(ThumbnailBatch) + Sync),
) -> Result<ThumbnailResult, String> {
    generate_with(ids, sources, cache, pool, &generate_thumbnail_file, send)
}

type Generator<'a> = &'a (dyn Fn(&str, &Path) -> Result<(), String> + Sync);

fn generate_with(
    ids: &[String],
    sources: &[String],
    cache: &Path,
    pool: &ThumbnailPool,
    generate: Generator<'_>,
    send: &(dyn Fn(ThumbnailBatch) + Sync),
) -> Result<ThumbnailResult, String> {
    fs::create_dir_all(cache).map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let total = ids.len().min(sources.len()) as u32;
    let mut result = ThumbnailResult::default();
    let mut jobs: Vec<Job> = Vec::new();
    let mut job_by_path: HashMap<PathBuf, usize> = HashMap::new();

    for (id, source) in ids.iter().zip(sources) {
        let thumb_path = cache.join(thumb_file_name(id));
        if let Some(&j) = job_by_path.get(&thumb_path) {
            jobs[j].ids.push(id);
        } else if thumb_path.exists() {
            result.items.push(ThumbnailEntry {
                image_id: id.clone(),
                thumbnail_path: thumb_path.to_string_lossy().into_owned(),
            });
        } else {
            job_by_path.insert(thumb_path.clone(), jobs.len());
            jobs.push(Job {
                source,
                thumb_path,
                ids: vec![id],
            });
        }
    }

    let mut completed = 0u32;
    for chunk in result.items.chunks(HIT_CHUNK) {
        completed += chunk.len() as u32;
        send(ThumbnailBatch {
            items: chunk.to_vec(),
            failed: Vec::new(),
            completed,
            total,
        });
    }
    if jobs.is_empty() {
        if total == 0 {
            send(ThumbnailBatch::default());
        }
        return Ok(result);
    }

    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(usize, Result<(), String>)>();
    let workers = pool.workers.min(jobs.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let (jobs, next) = (&jobs, &next);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(i) else { break };
                let slots = if is_video_path(job.source) {
                    &pool.video
                } else {
                    &pool.decode
                };
                let outcome = slots.run(|| generate(job.source, &job.thumb_path));
                if tx.send((i, outcome)).is_err() {
                    break;
                }
            });
        }
        drop(tx);

        // Collector: runs on this thread while the workers decode. Results
        // are flushed when FLUSH_ITEMS are pending or FLUSH_INTERVAL after the
        // oldest unsent one finished, independent of other jobs completing.
        let mut batch = ThumbnailBatch::default();
        let mut oldest_unsent: Option<Instant> = None;
        let flush = |batch: &mut ThumbnailBatch, completed: u32| {
            send(ThumbnailBatch {
                items: std::mem::take(&mut batch.items),
                failed: std::mem::take(&mut batch.failed),
                completed,
                total,
            });
        };
        loop {
            let received = match oldest_unsent {
                Some(t) => {
                    let wait = (t + FLUSH_INTERVAL).saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(r) => Some(r),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match rx.recv() {
                    Ok(r) => Some(r),
                    Err(_) => break,
                },
            };
            if let Some((i, outcome)) = received {
                let job = &jobs[i];
                completed += job.ids.len() as u32;
                for id in &job.ids {
                    match &outcome {
                        Ok(()) => batch.items.push(ThumbnailEntry {
                            image_id: (*id).clone(),
                            thumbnail_path: job.thumb_path.to_string_lossy().into_owned(),
                        }),
                        Err(_) => batch.failed.push((*id).clone()),
                    }
                }
                if let Err(e) = &outcome {
                    log::error!("Failed to generate thumbnail for {}: {}", job.ids[0], e);
                }
                oldest_unsent.get_or_insert_with(Instant::now);
            }
            let due = oldest_unsent.is_some_and(|t| t.elapsed() >= FLUSH_INTERVAL);
            if batch.items.len() + batch.failed.len() >= FLUSH_ITEMS || due {
                result.items.extend(batch.items.iter().cloned());
                result.failed.extend(batch.failed.iter().cloned());
                flush(&mut batch, completed);
                oldest_unsent = None;
            }
        }
        // All workers finished (or panicked, dropping their senders).
        if oldest_unsent.is_some() {
            result.items.extend(batch.items.iter().cloned());
            result.failed.extend(batch.failed.iter().cloned());
            flush(&mut batch, completed);
        }
    });

    Ok(result)
}

/// Thumbnails for a camera card. `source_paths` holds, per image, the file to
/// thumbnail: the RAF when there is one (its embedded preview decodes fastest),
/// otherwise the HIF/JPEG, or the movie.
#[tauri::command]
pub async fn generate_thumbnails(
    image_ids: Vec<String>,
    source_paths: Vec<String>,
    cache_dir: String,
    on_progress: Channel<ThumbnailBatch>,
) -> Result<ThumbnailResult, String> {
    tokio::task::spawn_blocking(move || {
        generate_thumbnail_batch(
            &image_ids,
            &source_paths,
            Path::new(&cache_dir),
            ThumbnailPool::global(),
            &|batch| {
                let _ = on_progress.send(batch);
            },
        )
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Get a single thumbnail path, generating it if needed.
#[tauri::command]
pub async fn get_thumbnail(
    image_id: String,
    raf_path: String,
    cache_dir: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let cache = Path::new(&cache_dir);
        fs::create_dir_all(cache).map_err(|e| format!("Failed to create cache dir: {}", e))?;

        let thumb_path = cache.join(thumb_file_name(&image_id));
        if !thumb_path.exists() {
            generate_thumbnail_file(&raf_path, &thumb_path)?;
        }
        Ok(thumb_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Cache file name for a thumbnail: `{id}.v3.jpg`.
///
/// The `.v3` generation marks lossy-JPEG thumbnails that render upright. Both
/// the camera and the library pipeline write this exact name, so a single
/// existence check per id decides hit vs. miss. Bumping the key invalidates
/// every cached thumbnail, so only do it when old files are actually wrong.
///
/// Files written before the ImageIO thumbnailer differ slightly but stay
/// valid: library thumbnails from `sips` were up to 600px on the long edge and
/// kept an EXIF orientation tag instead of rotated pixels (the webview applies
/// it); the new ones are 600px wide with the rotation in the pixels.
pub(crate) fn thumb_file_name(id: &str) -> String {
    format!("{}.v3.jpg", id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fuji_thumbs_{}_{}", tag, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_jpeg(path: &Path, w: u32, h: u32) {
        let img = RgbImage::from_pixel(w, h, Rgb([120, 80, 40]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        fs::write(path, out).unwrap();
    }

    /// A minimal RAF: magic, then the embedded JPEG's offset/length at 84..92.
    fn write_raf(path: &Path, jpeg: &[u8]) {
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(84, 0);
        let offset = 100u32;
        raf.extend_from_slice(&offset.to_be_bytes());
        raf.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
        raf.resize(offset as usize, 0);
        raf.extend_from_slice(jpeg);
        raf.extend_from_slice(&[0u8; 64]); // trailing "sensor data"
        fs::write(path, raf).unwrap();
    }

    fn collect(
        ids: &[String],
        sources: &[String],
        cache: &Path,
    ) -> (Vec<ThumbnailBatch>, ThumbnailResult) {
        let batches = Mutex::new(Vec::new());
        let result = generate_thumbnail_batch(
            ids,
            sources,
            cache,
            &ThumbnailPool::new(4, MAX_VIDEO_JOBS),
            &|b| batches.lock().unwrap().push(b),
        )
        .unwrap();
        (batches.into_inner().unwrap(), result)
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn thumb_file_name_uses_v3_jpg_key() {
        assert_eq!(thumb_file_name("DSCF1234"), "DSCF1234.v3.jpg");
        assert_eq!(thumb_file_name("100_0001"), "100_0001.v3.jpg");
    }

    #[test]
    fn recognizes_supported_video_extensions_case_insensitively() {
        assert!(is_video_path("/card/DCIM/DSCF0001.MOV"));
        assert!(is_video_path("/library/clip.mp4"));
        assert!(is_video_path("movie.m4v"));
        assert!(!is_video_path("DSCF0001.HIF"));
        assert!(is_raf_path("/card/DSCF0001.RAF"));
        assert!(is_raf_path("x.raf"));
        assert!(!is_raf_path("x.HIF"));
    }

    #[test]
    fn batch_generates_then_reports_cache_hits_in_one_message() {
        let src = temp_dir("src");
        let cache = temp_dir("cache");
        let mut ids = Vec::new();
        let mut sources = Vec::new();
        for i in 0..10 {
            let path = src.join(format!("IMG{}.JPG", i));
            write_jpeg(&path, 1200, 800);
            ids.push(format!("IMG{}", i));
            sources.push(path.to_string_lossy().into_owned());
        }
        // One RAF and one unreadable file.
        let mut embedded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut embedded, 90)
            .write_image(
                &vec![200u8; 900 * 600 * 3],
                900,
                600,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        write_raf(&src.join("RAW1.RAF"), &embedded);
        ids.push("RAW1".into());
        sources.push(src.join("RAW1.RAF").to_string_lossy().into_owned());
        fs::write(src.join("BAD.JPG"), b"nope").unwrap();
        ids.push("BAD".into());
        sources.push(src.join("BAD.JPG").to_string_lossy().into_owned());

        let (cold, result) = collect(&ids, &sources, &cache);
        let last = cold.last().unwrap();
        assert_eq!((last.completed, last.total), (12, 12));
        let generated: Vec<_> = cold.iter().flat_map(|b| b.items.iter()).collect();
        assert_eq!(generated.len(), 11, "everything but the broken file");
        assert!(generated.iter().all(|e| e.image_id != "BAD"));
        let failed: Vec<_> = cold.iter().flat_map(|b| b.failed.iter()).collect();
        assert_eq!(failed, vec!["BAD"]);
        // The return value carries the same complete outcome.
        assert_eq!(result.items.len(), 11);
        assert_eq!(result.failed, vec!["BAD".to_string()]);
        for entry in &generated {
            let img = image::open(&entry.thumbnail_path).unwrap();
            assert_eq!(img.width(), 600, "{}", entry.image_id);
        }
        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());

        // Warm: the 11 hits arrive in a single message; the broken file is
        // retried and fails again, producing one final progress message.
        let (warm, result) = collect(&ids, &sources, &cache);
        assert_eq!(warm.len(), 2);
        assert_eq!(warm[0].items.len(), 11);
        assert_eq!((warm[1].completed, warm[1].total), (12, 12));
        assert_eq!(warm[1].failed, vec!["BAD".to_string()]);
        assert_eq!((result.items.len(), result.failed.len()), (11, 1));

        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&cache).ok();
    }

    #[test]
    fn empty_batch_reports_once() {
        let cache = temp_dir("empty");
        let (batches, result) = collect(&[], &[], &cache);
        assert_eq!(batches.len(), 1);
        assert!(result.items.is_empty() && result.failed.is_empty());
        assert_eq!(batches[0].total, 0);
        fs::remove_dir_all(&cache).ok();
    }

    /// Runs `generate_with` with a fake generator, recording when each batch
    /// was sent.
    fn run_fake(
        ids: &[String],
        sources: &[String],
        cache: &Path,
        workers: usize,
        generate: Generator<'_>,
    ) -> (Vec<(Duration, ThumbnailBatch)>, ThumbnailResult) {
        let start = Instant::now();
        let batches = Mutex::new(Vec::new());
        let result = generate_with(
            ids,
            sources,
            cache,
            &ThumbnailPool::new(workers, MAX_VIDEO_JOBS),
            generate,
            &|b| batches.lock().unwrap().push((start.elapsed(), b)),
        )
        .unwrap();
        (batches.into_inner().unwrap(), result)
    }

    fn fake_write(source: &str, thumb: &Path) -> Result<(), String> {
        if source.contains("slow") {
            std::thread::sleep(Duration::from_millis(1500));
        }
        if source.contains("bad") {
            return Err("unreadable".into());
        }
        fs::write(thumb, b"jpeg").map_err(|e| e.to_string())
    }

    #[test]
    fn finished_results_are_sent_while_other_workers_are_busy() {
        let cache = temp_dir("busy");
        // Worker 1 is stuck on `slow` for 1.5 s; worker 2 finishes a, b and c
        // right away. Those must not wait for `slow`.
        let ids = strings(&["slow", "a", "b", "c"]);
        let (batches, result) = run_fake(&ids, &ids, &cache, 2, &fake_write);
        let (at, first) = &batches[0];
        assert_eq!(first.items.len(), 3, "a, b and c in the first batch");
        assert!(
            *at < Duration::from_millis(700),
            "first batch sent after {:?}",
            at
        );
        let (_, last) = batches.last().unwrap();
        assert_eq!(last.items[0].image_id, "slow");
        assert_eq!((last.completed, last.total), (4, 4));
        assert_eq!(result.items.len(), 4);
        fs::remove_dir_all(&cache).ok();
    }

    #[test]
    fn duplicate_ids_are_generated_once_and_all_reported() {
        let cache = temp_dir("dupes");
        let calls = AtomicUsize::new(0);
        let count = |source: &str, thumb: &Path| {
            calls.fetch_add(1, Ordering::Relaxed);
            fake_write(source, thumb)
        };
        let mut ids = vec!["DSCF0001".to_string(); 20];
        ids.push("DSCF0002".into());
        let sources: Vec<String> = (0..21).map(|i| format!("/folder{}/x.JPG", i)).collect();
        let (batches, result) = run_fake(&ids, &sources, &cache, 8, &count);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        let reported: usize = batches.iter().map(|(_, b)| b.items.len()).sum();
        assert_eq!(reported, 21);
        assert_eq!(result.items.len(), 21);
        assert_eq!(batches.last().unwrap().1.completed, 21);
        fs::remove_dir_all(&cache).ok();
    }

    #[test]
    fn duplicate_failures_are_all_reported() {
        let cache = temp_dir("dupe_fail");
        let ids = strings(&["X", "X", "Y"]);
        let sources = strings(&["/a/bad.JPG", "/b/bad.JPG", "/c/ok.JPG"]);
        let (batches, result) = run_fake(&ids, &sources, &cache, 2, &fake_write);
        assert_eq!(result.failed, strings(&["X", "X"]));
        assert_eq!(result.items.len(), 1);
        assert_eq!(batches.last().unwrap().1.completed, 3);
        fs::remove_dir_all(&cache).ok();
    }

    #[test]
    fn global_pool_limits_concurrent_decodes_across_calls() {
        let pool = ThumbnailPool::new(2, 1);
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let work = || {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            running.fetch_sub(1, Ordering::SeqCst);
        };
        std::thread::scope(|s| {
            for _ in 0..6 {
                s.spawn(|| pool.decode.run(work));
            }
        });
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }
}
