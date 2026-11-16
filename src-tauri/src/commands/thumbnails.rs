use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

use crate::imageio;
use crate::models::{ThumbnailBatch, ThumbnailEntry};
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
const MAX_VIDEO_JOBS: usize = 3;

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

/// Worker threads for thumbnails: all cores but two. Stills decode in-process
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

/// Counting semaphore bounding concurrent Quick Look runs.
struct Semaphore {
    free: Mutex<usize>,
    cv: Condvar,
}

impl Semaphore {
    fn new(n: usize) -> Self {
        Semaphore {
            free: Mutex::new(n),
            cv: Condvar::new(),
        }
    }

    fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        let mut free = self.free.lock().unwrap();
        while *free == 0 {
            free = self.cv.wait(free).unwrap();
        }
        *free -= 1;
        drop(free);
        let out = f();
        *self.free.lock().unwrap() += 1;
        self.cv.notify_one();
        out
    }
}

struct Pending {
    items: Vec<ThumbnailEntry>,
    completed: u32,
    last_flush: Instant,
}

/// Generate (or find cached) thumbnails `{id}.v3.jpg` in `cache` for each
/// `(id, source)` pair and report them through `send` in coalesced batches.
///
/// Cached thumbnails are reported first, in chunks of [`HIT_CHUNK`]. Missing
/// ones are then generated by `workers` threads that pull jobs in list order,
/// so the top of the grid (the list is newest first) fills in first.
pub(crate) fn generate_thumbnail_batch(
    ids: &[String],
    sources: &[String],
    cache: &Path,
    workers: usize,
    send: &(dyn Fn(ThumbnailBatch) + Sync),
) -> Result<(), String> {
    fs::create_dir_all(cache).map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let total = ids.len().min(sources.len()) as u32;
    let mut completed = 0u32;
    let mut hits: Vec<ThumbnailEntry> = Vec::new();
    let mut misses: Vec<(&String, &String, std::path::PathBuf)> = Vec::new();

    for (id, source) in ids.iter().zip(sources) {
        let thumb_path = cache.join(thumb_file_name(id));
        if thumb_path.exists() {
            hits.push(ThumbnailEntry {
                image_id: id.clone(),
                thumbnail_path: thumb_path.to_string_lossy().into_owned(),
            });
        } else {
            misses.push((id, source, thumb_path));
        }
    }

    for chunk in hits.chunks(HIT_CHUNK) {
        completed += chunk.len() as u32;
        send(ThumbnailBatch {
            items: chunk.to_vec(),
            completed,
            total,
        });
    }
    if misses.is_empty() {
        if total == 0 {
            send(ThumbnailBatch {
                items: Vec::new(),
                completed: 0,
                total: 0,
            });
        }
        return Ok(());
    }

    let next = AtomicUsize::new(0);
    let video_slots = Semaphore::new(MAX_VIDEO_JOBS);
    let pending = Mutex::new(Pending {
        items: Vec::new(),
        completed,
        last_flush: Instant::now(),
    });
    let flush = |p: &mut Pending| {
        p.last_flush = Instant::now();
        send(ThumbnailBatch {
            items: std::mem::take(&mut p.items),
            completed: p.completed,
            total,
        });
    };

    let workers = workers.clamp(1, misses.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((id, source, thumb_path)) = misses.get(i) else {
                    break;
                };
                let result = if is_video_path(source) {
                    video_slots.run(|| generate_thumbnail_file(source, thumb_path))
                } else {
                    generate_thumbnail_file(source, thumb_path)
                };
                let mut p = pending.lock().unwrap();
                p.completed += 1;
                match result {
                    Ok(()) => p.items.push(ThumbnailEntry {
                        image_id: (*id).clone(),
                        thumbnail_path: thumb_path.to_string_lossy().into_owned(),
                    }),
                    Err(e) => log::error!("Failed to generate thumbnail for {}: {}", id, e),
                }
                if p.items.len() >= FLUSH_ITEMS || p.last_flush.elapsed() >= FLUSH_INTERVAL {
                    flush(&mut p);
                }
            });
        }
    });

    let mut p = pending.lock().unwrap();
    if !p.items.is_empty() || p.completed != completed {
        flush(&mut p);
    }
    Ok(())
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
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        generate_thumbnail_batch(
            &image_ids,
            &source_paths,
            Path::new(&cache_dir),
            default_workers(),
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

    fn collect(ids: &[String], sources: &[String], cache: &Path) -> Vec<ThumbnailBatch> {
        let batches = Mutex::new(Vec::new());
        generate_thumbnail_batch(ids, sources, cache, 4, &|b| batches.lock().unwrap().push(b))
            .unwrap();
        batches.into_inner().unwrap()
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

        let cold = collect(&ids, &sources, &cache);
        let last = cold.last().unwrap();
        assert_eq!((last.completed, last.total), (12, 12));
        let generated: Vec<_> = cold.iter().flat_map(|b| b.items.iter()).collect();
        assert_eq!(generated.len(), 11, "everything but the broken file");
        assert!(generated.iter().all(|e| e.image_id != "BAD"));
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
        let warm = collect(&ids, &sources, &cache);
        assert_eq!(warm.len(), 2);
        assert_eq!(warm[0].items.len(), 11);
        assert_eq!((warm[1].completed, warm[1].total), (12, 12));

        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&cache).ok();
    }

    #[test]
    fn empty_batch_reports_once() {
        let cache = temp_dir("empty");
        let batches = collect(&[], &[], &cache);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].total, 0);
        fs::remove_dir_all(&cache).ok();
    }
}
