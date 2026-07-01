use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::index;

/// Maximum bytes to read when scanning for XMP Rating (256KB).
/// XMP is always near the start of HIF/RAF/JPEG files.
const READ_LIMIT: usize = 256 * 1024;

/// Bump when the ratings-cache entry shape changes so stale caches are ignored.
const RATINGS_CACHE_VERSION: u32 = 1;

/// Fingerprint of a source image plus its optional `.xmp` sidecar. A missing
/// file or sidecar contributes zeros, so the key still changes when either one
/// appears, disappears, or is edited (mtime/size).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RatingKey {
    mtime: u64,
    size: u64,
    sidecar_mtime: u64,
    sidecar_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RatingCacheEntry {
    key: RatingKey,
    /// `None` records that the file was scanned and has no rating, so the
    /// common (unrated) case is never re-scanned.
    rating: Option<u8>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RatingsCache {
    version: u32,
    /// Keyed by the absolute image path.
    entries: HashMap<String, RatingCacheEntry>,
}

/// `(mtime_secs, size)` for a path, or `(0, 0)` when it is absent/unreadable.
fn file_meta(path: &Path) -> (u64, u64) {
    match fs::metadata(path) {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (mtime, m.len())
        }
        Err(_) => (0, 0),
    }
}

/// Build the cache key from the image + its `.xmp` sidecar fingerprints.
fn rating_key(path: &Path) -> RatingKey {
    let (mtime, size) = file_meta(path);
    let (sidecar_mtime, sidecar_size) = file_meta(&path.with_extension("xmp"));
    RatingKey {
        mtime,
        size,
        sidecar_mtime,
        sidecar_size,
    }
}

/// Read the rating for a single file: `.xmp` sidecar first, then embedded XMP.
/// Returns `None` when the file carries no rating.
fn scan_rating(path: &Path, file_path: &str, attr_re: &Regex, elem_re: &Regex) -> Option<u8> {
    // 1. Check for .xmp sidecar file first (takes priority — user may have written it)
    let sidecar_path = path.with_extension("xmp");
    if sidecar_path.exists() {
        if let Ok(sidecar_content) = fs::read_to_string(&sidecar_path) {
            if let Some(r) = extract_rating(&sidecar_content, attr_re, elem_re) {
                if (1..=5).contains(&r) {
                    return Some(r);
                }
            }
        }
    }

    // 2. Read embedded XMP from the image file
    if !path.exists() {
        return None;
    }

    let data = read_file_head(file_path, READ_LIMIT).ok()?;

    // Extract the XMP packet as a string (XMP is always valid UTF-8/ASCII)
    let xmp_str = extract_xmp_string(&data)?;
    let r = extract_rating(&xmp_str, attr_re, elem_re)?;
    if (1..=5).contains(&r) {
        Some(r)
    } else {
        None
    }
}

// Byte patterns for XMP packet markers (ASCII, safe to search in binary data)
const XPACKET_BEGIN: &[u8] = b"<?xpacket begin=";
const XPACKET_END: &[u8] = b"<?xpacket end=";
const XPACKET_CLOSE: &[u8] = b"?>";

/// Find a byte pattern in a byte slice. Returns the starting index.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Read XMP ratings from multiple files.
/// Returns a map of filename stem → rating (1-5). Files without ratings are omitted.
/// Checks both embedded XMP in the image file AND .xmp sidecar files.
///
/// Results are cached in `<cache_dir>/ratings-cache.json`, keyed by each file's
/// path + mtime + size (and its sidecar's). Unchanged files — including unrated
/// ones, recorded as `None` — skip the 256 KB read + XMP parse entirely, so the
/// common case of re-opening a library never re-scans.
pub fn read_ratings(file_paths: &[String], cache_dir: &Path) -> Result<HashMap<String, u8>, String> {
    let attr_re = Regex::new(r#"xmp:Rating="(\d)"#)
        .map_err(|e| format!("Failed to compile regex: {}", e))?;
    let elem_re = Regex::new(r#"<xmp:Rating>(\d)</xmp:Rating>"#)
        .map_err(|e| format!("Failed to compile regex: {}", e))?;

    // Load the previous cache (empty if missing or from an older version).
    let cache_path = cache_dir.join("ratings-cache.json");
    let previous: RatingsCache = index::read_json(&cache_path)
        .ok()
        .filter(|c: &RatingsCache| c.version == RATINGS_CACHE_VERSION)
        .unwrap_or_default();

    // Regexes and the cache are Sync/read-only, so workers share them while
    // scanning files in parallel. Each path yields exactly one cache entry —
    // reused on a fingerprint hit, otherwise re-read — so the persisted cache
    // stays complete (including unrated files recorded as `None`).
    let entries: Vec<(String, RatingCacheEntry)> = file_paths
        .par_iter()
        .map(|file_path| {
            let path = Path::new(file_path);
            let key = rating_key(path);

            if let Some(hit) = previous.entries.get(file_path) {
                if hit.key == key {
                    return (file_path.clone(), hit.clone());
                }
            }

            let rating = scan_rating(path, file_path, &attr_re, &elem_re);
            (file_path.clone(), RatingCacheEntry { key, rating })
        })
        .collect();

    // Build the stem → rating result and the fresh cache to persist.
    let mut ratings = HashMap::new();
    let mut next = RatingsCache {
        version: RATINGS_CACHE_VERSION,
        entries: HashMap::with_capacity(entries.len()),
    };
    for (file_path, entry) in entries {
        if let Some(r) = entry.rating {
            let stem = Path::new(&file_path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            ratings.insert(stem, r);
        }
        next.entries.insert(file_path, entry);
    }

    // Persist best-effort — a cache write failure must not fail the read.
    if let Err(e) = fs::create_dir_all(cache_dir) {
        log::warn!("Failed to create ratings cache dir: {}", e);
    } else if let Err(e) = index::write_json(&cache_path, &next) {
        log::warn!("Failed to write ratings cache: {}", e);
    }

    Ok(ratings)
}

/// Extract the XMP packet from binary data as a UTF-8 string.
/// Uses byte-level searching so positions are correct even in binary files.
fn extract_xmp_string(data: &[u8]) -> Option<String> {
    let start = find_bytes(data, XPACKET_BEGIN)?;
    let end_marker = find_bytes(&data[start..], XPACKET_END)?;
    let end_abs = start + end_marker;
    let close = find_bytes(&data[end_abs..], XPACKET_CLOSE)
        .map(|pos| end_abs + pos + XPACKET_CLOSE.len())
        .unwrap_or(end_abs);

    // XMP packets are XML — valid UTF-8
    String::from_utf8(data[start..close].to_vec()).ok()
}

/// Extract XMP Rating value from a string containing XMP data.
fn extract_rating(content: &str, attr_re: &Regex, elem_re: &Regex) -> Option<u8> {
    // Try attribute form: xmp:Rating="N"
    if let Some(caps) = attr_re.captures(content) {
        if let Some(m) = caps.get(1) {
            if let Ok(v) = m.as_str().parse::<u8>() {
                return Some(v);
            }
        }
    }

    // Try element form: <xmp:Rating>N</xmp:Rating>
    if let Some(caps) = elem_re.captures(content) {
        if let Some(m) = caps.get(1) {
            if let Ok(v) = m.as_str().parse::<u8>() {
                return Some(v);
            }
        }
    }

    None
}

/// Read the first `limit` bytes of a file.
fn read_file_head(file_path: &str, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file =
        fs::File::open(file_path).map_err(|e| format!("Failed to open {}: {}", file_path, e))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Failed to read metadata for {}: {}", file_path, e))?;
    let read_size = std::cmp::min(limit, metadata.len() as usize);
    let mut buffer = vec![0u8; read_size];
    file.read_exact(&mut buffer)
        .map_err(|e| format!("Failed to read {}: {}", file_path, e))?;
    Ok(buffer)
}

/// Write an XMP Rating into a single file.
/// - If the file already has an XMP packet with a Rating, updates it in-place.
/// - If the file has an XMP packet without a Rating, inserts one using padding.
/// - If the file has no XMP packet, writes an XMP sidecar (.xmp) file.
///
/// All byte position arithmetic uses the raw byte array, not lossy string conversion,
/// so positions are correct even for binary image files (HIF, RAF, etc.).
pub fn write_rating(file_path: &str, rating: u8) -> Result<(), String> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    // Read only the file head. XMP packets in HIF/RAF always live near the start.
    // 1 MB is comfortably larger than any real-world XMP packet (usually <10 KB).
    let data = read_file_head(file_path, 1 << 20)?;

    // Find XMP packet boundaries using byte-level search
    let pkt_start = match find_bytes(&data, XPACKET_BEGIN) {
        Some(pos) => pos,
        None => return write_sidecar(file_path, rating),
    };

    let pkt_end_marker = match find_bytes(&data[pkt_start..], XPACKET_END) {
        Some(pos) => pkt_start + pos,
        None => return write_sidecar(file_path, rating),
    };

    let pkt_end = find_bytes(&data[pkt_end_marker..], XPACKET_CLOSE)
        .map(|pos| pkt_end_marker + pos + XPACKET_CLOSE.len())
        .unwrap_or(pkt_end_marker);

    // Extract the XMP packet as a string (XMP is valid UTF-8)
    let xmp_bytes = &data[pkt_start..pkt_end];
    let xmp_str = String::from_utf8(xmp_bytes.to_vec())
        .map_err(|_| "XMP packet is not valid UTF-8".to_string())?;

    let original_len = xmp_bytes.len();
    let rating_char = (b'0' + rating) as char;

    // Case 1: Replace existing Rating attribute — xmp:Rating="N"
    let attr_re = Regex::new(r#"xmp:Rating="\d""#).unwrap();
    if attr_re.is_match(&xmp_str) {
        let new_xmp = attr_re
            .replace(&xmp_str, format!("xmp:Rating=\"{}\"", rating_char))
            .to_string();
        return write_xmp_back(&data, file_path, pkt_start, original_len, &new_xmp);
    }

    // Case 2: Replace existing Rating element — <xmp:Rating>N</xmp:Rating>
    let elem_re = Regex::new(r#"<xmp:Rating>\d</xmp:Rating>"#).unwrap();
    if elem_re.is_match(&xmp_str) {
        let new_xmp = elem_re
            .replace(
                &xmp_str,
                format!("<xmp:Rating>{}</xmp:Rating>", rating_char),
            )
            .to_string();
        return write_xmp_back(&data, file_path, pkt_start, original_len, &new_xmp);
    }

    // Case 3: Rating doesn't exist — insert it into rdf:Description, consuming padding
    let insert_attr = format!("\n      xmp:Rating=\"{}\"", rating_char);
    let needs_ns = !xmp_str.contains("xmlns:xmp=");
    let ns_decl = if needs_ns {
        " xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\""
    } else {
        ""
    };
    let full_insert = format!("{}{}", ns_decl, insert_attr);

    // Find rdf:Description to insert attribute before the closing > or />
    let desc_pos = match xmp_str.find("rdf:Description") {
        Some(pos) => pos,
        None => return write_sidecar(file_path, rating),
    };

    let desc_rest = &xmp_str[desc_pos..];
    let tag_end = desc_rest
        .find('>')
        .or_else(|| desc_rest.find("/>"))
        .unwrap_or(desc_rest.len());
    let insert_pos = desc_pos + tag_end;

    // Build new XMP with the inserted attribute
    let mut new_xmp = String::with_capacity(xmp_str.len() + full_insert.len());
    new_xmp.push_str(&xmp_str[..insert_pos]);
    new_xmp.push_str(&full_insert);
    new_xmp.push_str(&xmp_str[insert_pos..]);

    // Trim padding to maintain original packet size.
    // Padding is whitespace before <?xpacket end=
    let excess = new_xmp.len() as isize - original_len as isize;
    if excess > 0 {
        let excess = excess as usize;
        if let Some(end_idx) = new_xmp.find("<?xpacket end=") {
            // Walk backwards from end marker to find padding
            let before_end = &new_xmp[..end_idx];
            let trimmed_end = before_end.trim_end().len();
            let available_padding = end_idx - trimmed_end;

            if available_padding >= excess {
                // Remove `excess` bytes from the end of padding
                let cut_start = end_idx - excess;
                let trimmed_xmp = format!("{}{}", &new_xmp[..cut_start], &new_xmp[end_idx..]);

                if trimmed_xmp.len() == original_len {
                    return write_xmp_back(&data, file_path, pkt_start, original_len, &trimmed_xmp);
                }
            }
        }
    }

    // Padding adjustment failed — fall back to sidecar
    write_sidecar(file_path, rating)
}

/// Patch a modified XMP packet into the file at its original byte range.
/// The new XMP must be exactly `original_len` bytes, so file size is unchanged.
///
/// Writes ~5 KB in place instead of rewriting the entire (50-100 MB) image file —
/// critical for responsiveness when rating photos. Safer, too: a crash mid-write
/// corrupts at most the XMP packet (which readers skip gracefully), not image data.
fn write_xmp_back(
    _data: &[u8], // unused: in-place seek+write doesn't need the rest of the file
    file_path: &str,
    pkt_start: usize,
    original_len: usize,
    new_xmp: &str,
) -> Result<(), String> {
    use std::io::{Seek, SeekFrom, Write};

    let new_bytes = new_xmp.as_bytes();

    // For same-length replacements (Case 1 & 2), sizes match exactly.
    // For insertions with padding trimming, we've ensured the size matches.
    if new_bytes.len() != original_len {
        // Sizes don't match — fall back to sidecar to be safe
        return write_sidecar(file_path, new_xmp.contains("xmp:Rating=\"")
            .then(|| {
                // Extract rating from the new XMP
                let re = Regex::new(r#"xmp:Rating="(\d)""#).unwrap();
                re.captures(new_xmp)
                    .and_then(|c| c.get(1))
                    .and_then(|m| m.as_str().parse::<u8>().ok())
                    .unwrap_or(0)
            })
            .unwrap_or(0));
    }

    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(file_path)
        .map_err(|e| format!("Failed to open {} for writing: {}", file_path, e))?;
    f.seek(SeekFrom::Start(pkt_start as u64))
        .map_err(|e| format!("Failed to seek in {}: {}", file_path, e))?;
    f.write_all(new_bytes)
        .map_err(|e| format!("Failed to write {}: {}", file_path, e))?;
    f.sync_data()
        .map_err(|e| format!("Failed to fsync {}: {}", file_path, e))?;
    Ok(())
}

/// Write an XMP sidecar file (.xmp) alongside the image file.
fn write_sidecar(image_path: &str, rating: u8) -> Result<(), String> {
    let path = Path::new(image_path);
    let xmp_path = path.with_extension("xmp");

    let content = format!(
        r#"<?xpacket begin="{}" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:xmp="http://ns.adobe.com/xap/1.0/"
      xmp:Rating="{}"/>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
        '\u{feff}', rating
    );

    fs::write(&xmp_path, content).map_err(|e| {
        format!(
            "Failed to write XMP sidecar {}: {}",
            xmp_path.display(),
            e
        )
    })
}

/// Write XMP ratings to multiple files in batch.
pub fn write_ratings_batch(file_ratings: &[(String, u8)]) -> Result<(), String> {
    for (file_path, rating) in file_ratings {
        if let Err(e) = write_rating(file_path, *rating) {
            log::warn!("Failed to write rating for {}: {}", file_path, e);
            // Continue with other files, don't fail the entire batch
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fuji_{}_{}", tag, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) {
        let mut f = fs::File::create(dir.join(name)).unwrap();
        f.write_all(bytes).unwrap();
    }

    // The cache key must be stable for an unchanged file and change whenever the
    // image or its sidecar's mtime/size shifts — otherwise the cache would serve
    // stale ratings (mismatch) or needlessly re-scan (spurious mismatch).
    #[test]
    fn rating_key_matches_until_file_or_sidecar_changes() {
        let dir = unique_temp_dir("ratekey");
        let img = dir.join("shot.HIF");
        write_file(&dir, "shot.HIF", b"aaaa");

        let k1 = rating_key(&img);
        assert_eq!(k1, rating_key(&img), "unchanged file → identical key");

        // Grow the file: size changes → key changes.
        write_file(&dir, "shot.HIF", b"aaaabbbbcccc");
        let k2 = rating_key(&img);
        assert_ne!(k1, k2, "size change → new key");

        // A newly-appearing sidecar changes the key.
        write_file(&dir, "shot.xmp", br#"<x xmp:Rating="3"/>"#);
        let k3 = rating_key(&img);
        assert_ne!(k2, k3, "sidecar appearance → new key");

        fs::remove_dir_all(&dir).ok();
    }

    // Rated files come back by stem, unrated files are recorded as `None`, and a
    // second read is served from the persisted cache.
    #[test]
    fn ratings_cache_records_rated_and_unrated() {
        let dir = unique_temp_dir("ratecache");
        let cache = unique_temp_dir("ratecachedir");
        write_file(&dir, "rated.HIF", b"stub");
        write_file(&dir, "rated.xmp", br#"<?xpacket?><x xmp:Rating="4"/>"#);
        write_file(&dir, "plain.HIF", b"no-xmp-here");
        let rated = dir.join("rated.HIF").to_string_lossy().to_string();
        let plain = dir.join("plain.HIF").to_string_lossy().to_string();

        let result = read_ratings(&[rated.clone(), plain.clone()], &cache).unwrap();
        assert_eq!(result.get("rated"), Some(&4));
        assert_eq!(result.get("plain"), None);

        let persisted: RatingsCache =
            index::read_json(&cache.join("ratings-cache.json")).unwrap();
        assert_eq!(persisted.version, RATINGS_CACHE_VERSION);
        assert_eq!(persisted.entries.get(&rated).unwrap().rating, Some(4));
        // Unrated file is recorded with `None` so it is never re-scanned.
        assert!(persisted.entries.contains_key(&plain));
        assert_eq!(persisted.entries.get(&plain).unwrap().rating, None);

        // Second read (cache hit for both) returns identical results.
        let again = read_ratings(&[rated, plain], &cache).unwrap();
        assert_eq!(again.get("rated"), Some(&4));
        assert_eq!(again.get("plain"), None);

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&cache).ok();
    }
}
