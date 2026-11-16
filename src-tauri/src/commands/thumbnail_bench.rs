//! Thumbnail benchmark over real camera files. Ignored by default; run with
//!
//! ```sh
//! FUJI_THUMB_BENCH_DIR=/path/to/samples \
//!   cargo test --release --lib thumbnail_bench -- --ignored --nocapture
//! ```
//!
//! Every file in the sample dir (non-recursive) is grouped by extension. For
//! each group and each worker count, thumbnails are generated cold into a fresh
//! temp cache `FUJI_THUMB_BENCH_RUNS` times (default 3), reporting the median
//! wall time. Each output is checked: it decodes with the `image` crate, is at
//! most 600px wide, has the source's displayed (rotated) aspect ratio, and is
//! not a blank frame.
//! Finally a warm run over a synthetic 3000-entry cache counts progress
//! messages. Optional env:
//! - `FUJI_THUMB_BENCH_WORKERS`: comma-separated worker counts (default `4,10`)
//! - `FUJI_THUMB_BENCH_KEEP`: directory to copy one run's thumbnails into

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tauri::ipc::Channel;

use super::thumbnails::{generate_thumbnail_batch, thumb_file_name, THUMBNAIL_MAX_WIDTH};
use crate::imageio;
use crate::models::ThumbnailBatch;
use crate::raf::preview;

fn env_list(name: &str, default: &str) -> Vec<usize> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

fn fresh_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("fuji_bench_{}_{}", tag, nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Run one batch through a real IPC `Channel` (so serialization is included)
/// and return (wall seconds, messages sent).
fn run_batch(ids: &[String], sources: &[String], cache: &Path, workers: usize) -> (f64, usize) {
    let messages = Arc::new(AtomicUsize::new(0));
    let counter = messages.clone();
    let channel: Channel<ThumbnailBatch> = Channel::new(move |_body| {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let start = Instant::now();
    generate_thumbnail_batch(ids, sources, cache, workers, &|batch| {
        let _ = channel.send(batch);
    })
    .unwrap();
    (
        start.elapsed().as_secs_f64(),
        messages.load(Ordering::Relaxed),
    )
}

fn displayed_size(source: &Path) -> (u32, u32) {
    let is_raf = source
        .extension()
        .map(|e| e.eq_ignore_ascii_case("raf"))
        .unwrap_or(false);
    if is_raf {
        let jpeg = preview::extract_jpeg_from_raf(source).unwrap();
        imageio::oriented_size(imageio::Source::Bytes(&jpeg)).unwrap()
    } else {
        imageio::oriented_size(imageio::Source::File(source)).unwrap()
    }
}

fn check_outputs(ids: &[String], sources: &[String], cache: &Path) {
    for (id, source) in ids.iter().zip(sources) {
        let thumb = cache.join(thumb_file_name(id));
        if super::thumbnails::is_video_path(source) {
            let img = image::open(&thumb).unwrap_or_else(|e| panic!("{}: {}", id, e));
            assert!(img.width() <= THUMBNAIL_MAX_WIDTH && img.height() <= THUMBNAIL_MAX_WIDTH);
            continue;
        }
        let img = image::open(&thumb).unwrap_or_else(|e| panic!("{}: {}", id, e));
        // Real photo content, not a blank frame: luma must vary.
        let luma = img.to_luma8();
        let n = luma.len() as f64;
        let mean = luma.iter().map(|&v| v as f64).sum::<f64>() / n;
        let var = luma.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n;
        assert!(
            var.sqrt() > 4.0,
            "{}: near-uniform output (sd {:.1})",
            id,
            var.sqrt()
        );
        let (sw, sh) = displayed_size(Path::new(source));
        let expected_w = sw.min(THUMBNAIL_MAX_WIDTH);
        assert_eq!(img.width(), expected_w, "{} width", id);
        let expected_h = sh as f64 * expected_w as f64 / sw as f64;
        assert!(
            (img.height() as f64 - expected_h).abs() <= 2.0,
            "{}: {}x{} vs displayed {}x{}",
            id,
            img.width(),
            img.height(),
            sw,
            sh
        );
    }
}

#[test]
#[ignore]
fn thumbnail_bench() {
    let Ok(dir) = std::env::var("FUJI_THUMB_BENCH_DIR") else {
        eprintln!("FUJI_THUMB_BENCH_DIR not set; skipping");
        return;
    };
    let runs = env_list("FUJI_THUMB_BENCH_RUNS", "3")[0].max(1);
    let worker_counts = env_list("FUJI_THUMB_BENCH_WORKERS", "4,10");
    let keep = std::env::var("FUJI_THUMB_BENCH_KEEP")
        .ok()
        .map(PathBuf::from);

    let mut groups: std::collections::BTreeMap<String, Vec<PathBuf>> = Default::default();
    for entry in fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            groups
                .entry(ext.to_string_lossy().to_uppercase())
                .or_default()
                .push(path);
        }
    }

    println!("group  files  workers  median_s  min_s  ms/file  messages");
    for (ext, mut files) in groups {
        files.sort();
        let ids: Vec<String> = files
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        let sources: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        // Warm the OS file cache so every run reads from memory.
        for f in &files {
            let _ = fs::read(f);
        }
        for &workers in &worker_counts {
            let mut times = Vec::new();
            let mut msgs = 0;
            for run in 0..runs {
                let cache = fresh_dir("cold");
                let (t, m) = run_batch(&ids, &sources, &cache, workers);
                times.push(t);
                msgs = m;
                if run == 0 {
                    check_outputs(&ids, &sources, &cache);
                    if let Some(keep) = &keep {
                        let dst = keep.join(&ext);
                        fs::create_dir_all(&dst).unwrap();
                        for id in &ids {
                            let name = thumb_file_name(id);
                            let _ = fs::copy(cache.join(&name), dst.join(&name));
                        }
                    }
                }
                fs::remove_dir_all(&cache).ok();
            }
            let min = times.iter().cloned().fold(f64::MAX, f64::min);
            let med = median(times);
            println!(
                "{:<6} {:>5}  {:>7}  {:>8.3}  {:>5.3}  {:>7.1}  {:>8}",
                ext,
                files.len(),
                workers,
                med,
                min,
                med * 1000.0 / files.len() as f64,
                msgs
            );
        }
    }

    // Warm path: 3000 ids, all cached.
    let cache = fresh_dir("warm");
    let seed = image::RgbImage::from_pixel(600, 400, image::Rgb([90, 90, 90]));
    let seed_path = cache.join("seed.jpg");
    seed.save(&seed_path).unwrap();
    let ids: Vec<String> = (0..3000).map(|i| format!("DSCF{:04}", i)).collect();
    for id in &ids {
        fs::copy(&seed_path, cache.join(thumb_file_name(id))).unwrap();
    }
    let sources: Vec<String> = ids
        .iter()
        .map(|id| format!("/nonexistent/{}.HIF", id))
        .collect();
    let mut times = Vec::new();
    let mut msgs = 0;
    for _ in 0..runs.max(5) {
        let (t, m) = run_batch(&ids, &sources, &cache, 10);
        times.push(t);
        msgs = m;
    }
    println!(
        "warm   {:>5}  {:>7}  {:>8.4}  {:>5}  {:>7}  {:>8}",
        ids.len(),
        10,
        median(times),
        "",
        "",
        msgs
    );
    fs::remove_dir_all(&cache).ok();
}
