//! In-process JPEG thumbnails on macOS ImageIO.
//!
//! `CGImageSourceCreateThumbnailAtIndex` decodes at reduced size where the
//! codec allows it (JPEG scaled IDCT, HEIF tiles) instead of decoding every
//! pixel and resizing afterwards, and it applies the EXIF orientation for us.
//! The result is encoded with `CGImageDestination` as a baseline JPEG with no
//! orientation tag, so the `image` crate (CLIP indexer) reads it upright.
//!
//! Raw FFI against CoreFoundation/ImageIO keeps this dependency-free; the
//! surface used here is small and every `Create`/`Copy` result is released by
//! the `Cf` guard.

use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

type CFTypeRef = *const c_void;
type CFIndex = isize;

#[repr(C)]
struct Opaque {
    _p: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFAllocatorNull: CFTypeRef;
    static kCFBooleanTrue: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;

    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreateWithBytesNoCopy(
        alloc: CFTypeRef,
        bytes: *const u8,
        length: CFIndex,
        dealloc: CFTypeRef,
    ) -> CFTypeRef;
    fn CFDataCreateMutable(alloc: CFTypeRef, capacity: CFIndex) -> CFTypeRef;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: CFIndex,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFNumberCreate(alloc: CFTypeRef, the_type: CFIndex, value: *const c_void) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, the_type: CFIndex, value: *mut c_void) -> u8;
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: CFIndex,
        encoding: u32,
        external: u8,
    ) -> CFTypeRef;
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: CFTypeRef,
        buffer: *const u8,
        len: CFIndex,
        is_directory: u8,
    ) -> CFTypeRef;
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    static kCGImageSourceThumbnailMaxPixelSize: CFTypeRef;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFTypeRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFTypeRef;
    static kCGImageDestinationLossyCompressionQuality: CFTypeRef;
    static kCGImagePropertyPixelWidth: CFTypeRef;
    static kCGImagePropertyPixelHeight: CFTypeRef;
    static kCGImagePropertyOrientation: CFTypeRef;

    fn CGImageSourceCreateWithURL(url: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceCreateWithData(data: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    fn CGImageDestinationCreateWithData(
        data: CFTypeRef,
        uti: CFTypeRef,
        count: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    fn CGImageDestinationAddImage(dest: CFTypeRef, image: CFTypeRef, properties: CFTypeRef);
    fn CGImageDestinationFinalize(dest: CFTypeRef) -> bool;
}

#[link(name = "objc")]
extern "C" {
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

const K_CF_NUMBER_SINT64: CFIndex = 4;
const K_CF_NUMBER_FLOAT64: CFIndex = 6;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// Owned CoreFoundation reference, released on drop.
struct Cf(CFTypeRef);

impl Cf {
    fn new(ptr: CFTypeRef, what: &str) -> Result<Self, String> {
        if ptr.is_null() {
            Err(format!("ImageIO: {} failed", what))
        } else {
            Ok(Cf(ptr))
        }
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

/// Drains anything ImageIO autoreleases. The worker threads are long-lived
/// plain threads without a run loop, so nothing else would drain it.
struct AutoreleasePool(*mut c_void);

impl AutoreleasePool {
    fn new() -> Self {
        AutoreleasePool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

fn cf_i64(value: i64) -> Result<Cf, String> {
    Cf::new(
        unsafe {
            CFNumberCreate(
                std::ptr::null(),
                K_CF_NUMBER_SINT64,
                &value as *const i64 as _,
            )
        },
        "CFNumberCreate",
    )
}

fn cf_f64(value: f64) -> Result<Cf, String> {
    Cf::new(
        unsafe {
            CFNumberCreate(
                std::ptr::null(),
                K_CF_NUMBER_FLOAT64,
                &value as *const f64 as _,
            )
        },
        "CFNumberCreate",
    )
}

fn cf_dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Result<Cf, String> {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    Cf::new(
        unsafe {
            CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                pairs.len() as CFIndex,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        },
        "CFDictionaryCreate",
    )
}

fn dict_i64(dict: CFTypeRef, key: CFTypeRef) -> Option<i64> {
    unsafe {
        let value = CFDictionaryGetValue(dict, key);
        if value.is_null() {
            return None;
        }
        let mut out: i64 = 0;
        (CFNumberGetValue(value, K_CF_NUMBER_SINT64, &mut out as *mut i64 as _) != 0).then_some(out)
    }
}

/// Where the encoded image comes from.
pub enum Source<'a> {
    /// A file ImageIO can open (HEIF/HEIC/HIF, JPEG, ...).
    File(&'a Path),
    /// Encoded bytes already in memory (the JPEG embedded in a RAF).
    Bytes(&'a [u8]),
}

/// Create an image source. For `Source::Bytes` the data is not copied, so
/// the returned source must not outlive the borrowed bytes.
unsafe fn open_source(source: &Source<'_>) -> Result<Cf, String> {
    match source {
        Source::File(path) => {
            let bytes = path.as_os_str().as_bytes();
            let url = Cf::new(
                CFURLCreateFromFileSystemRepresentation(
                    std::ptr::null(),
                    bytes.as_ptr(),
                    bytes.len() as CFIndex,
                    0,
                ),
                "CFURLCreateFromFileSystemRepresentation",
            )?;
            Cf::new(
                CGImageSourceCreateWithURL(url.0, std::ptr::null()),
                "CGImageSourceCreateWithURL",
            )
        }
        Source::Bytes(bytes) => {
            let data = Cf::new(
                CFDataCreateWithBytesNoCopy(
                    std::ptr::null(),
                    bytes.as_ptr(),
                    bytes.len() as CFIndex,
                    kCFAllocatorNull,
                ),
                "CFDataCreateWithBytesNoCopy",
            )?;
            Cf::new(
                CGImageSourceCreateWithData(data.0, std::ptr::null()),
                "CGImageSourceCreateWithData",
            )
        }
    }
}

/// Pixel size after applying the EXIF orientation (5..=8 swap the axes).
unsafe fn oriented_dimensions(image_source: CFTypeRef) -> Result<(i64, i64), String> {
    // Header-only read; no pixels are decoded.
    let props = Cf::new(
        CGImageSourceCopyPropertiesAtIndex(image_source, 0, std::ptr::null()),
        "CGImageSourceCopyPropertiesAtIndex (unreadable image)",
    )?;
    let w = dict_i64(props.0, kCGImagePropertyPixelWidth).unwrap_or(0);
    let h = dict_i64(props.0, kCGImagePropertyPixelHeight).unwrap_or(0);
    let orientation = dict_i64(props.0, kCGImagePropertyOrientation).unwrap_or(1);
    Ok(if (5..=8).contains(&orientation) {
        (h, w)
    } else {
        (w, h)
    })
}

/// Size limit for `kCGImageSourceThumbnailMaxPixelSize` so that the oriented
/// output is at most `max_width` wide. ImageIO limits the long edge; a
/// portrait 3:2 shot therefore needs a 900px long edge to come out 600 wide.
fn max_pixel_size_for_width(oriented_w: i64, oriented_h: i64, max_width: u32) -> i64 {
    let max_width = max_width as i64;
    if oriented_w <= 0 || oriented_h <= 0 {
        return max_width;
    }
    if oriented_w <= max_width {
        // Never upscale: the long edge of the source itself.
        return oriented_w.max(oriented_h);
    }
    if oriented_h <= oriented_w {
        max_width
    } else {
        // Round up so integer truncation inside ImageIO cannot land at 599.
        (oriented_h * max_width + oriented_w - 1) / oriented_w
    }
}

/// Decode `source` at reduced size, bake in EXIF orientation, and encode a
/// JPEG at most `max_width` pixels wide (after rotation) at `quality` (0..1).
pub fn jpeg_thumbnail(source: Source<'_>, max_width: u32, quality: f64) -> Result<Vec<u8>, String> {
    let _pool = AutoreleasePool::new();
    unsafe {
        let image_source = open_source(&source)?;

        let (ow, oh) = oriented_dimensions(image_source.0)?;

        let max_px = cf_i64(max_pixel_size_for_width(ow, oh, max_width))?;

        // FromImageAlways: always decode the main image. Fuji HEIF/HIF files
        // embed a 1920x1280 10-bit preview that would be ~10x faster, but
        // ImageIO returns it as an all-black image, and JPEGs only embed a
        // 160x120 EXIF thumbnail.
        let options = cf_dict(&[
            (kCGImageSourceThumbnailMaxPixelSize, max_px.0),
            (kCGImageSourceCreateThumbnailFromImageAlways, kCFBooleanTrue),
            (kCGImageSourceCreateThumbnailWithTransform, kCFBooleanTrue),
        ])?;
        let image = Cf::new(
            CGImageSourceCreateThumbnailAtIndex(image_source.0, 0, options.0),
            "CGImageSourceCreateThumbnailAtIndex",
        )?;

        let out = Cf::new(
            CFDataCreateMutable(std::ptr::null(), 0),
            "CFDataCreateMutable",
        )?;
        let uti_str = b"public.jpeg";
        let uti = Cf::new(
            CFStringCreateWithBytes(
                std::ptr::null(),
                uti_str.as_ptr(),
                uti_str.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
                0,
            ),
            "CFStringCreateWithBytes",
        )?;
        let dest = Cf::new(
            CGImageDestinationCreateWithData(out.0, uti.0, 1, std::ptr::null()),
            "CGImageDestinationCreateWithData",
        )?;
        let q = cf_f64(quality)?;
        // Only the quality key: no orientation tag is written (the transform is
        // already in the pixels) and the JPEG stays baseline.
        let dest_props = cf_dict(&[(kCGImageDestinationLossyCompressionQuality, q.0)])?;
        CGImageDestinationAddImage(dest.0, image.0, dest_props.0);
        if !CGImageDestinationFinalize(dest.0) {
            return Err("ImageIO: JPEG encode failed".to_string());
        }
        drop(dest);

        let len = CFDataGetLength(out.0) as usize;
        let ptr = CFDataGetBytePtr(out.0);
        if ptr.is_null() || len == 0 {
            return Err("ImageIO: empty JPEG output".to_string());
        }
        Ok(std::slice::from_raw_parts(ptr, len).to_vec())
    }
}

/// Displayed (orientation-applied) pixel size of an image, from its header.
#[cfg(test)]
pub(crate) fn oriented_size(source: Source<'_>) -> Option<(u32, u32)> {
    let _pool = AutoreleasePool::new();
    unsafe {
        let image_source = open_source(&source).ok()?;
        let (w, h) = oriented_dimensions(image_source.0).ok()?;
        Some((w as u32, h as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage};

    #[test]
    fn size_limit_targets_width() {
        assert_eq!(max_pixel_size_for_width(6000, 4000, 600), 600);
        assert_eq!(max_pixel_size_for_width(4000, 6000, 600), 900);
        assert_eq!(max_pixel_size_for_width(5152, 7728, 600), 900);
        // Square and small sources.
        assert_eq!(max_pixel_size_for_width(1000, 1000, 600), 600);
        assert_eq!(max_pixel_size_for_width(400, 300, 600), 400);
        assert_eq!(max_pixel_size_for_width(300, 500, 600), 500);
        assert_eq!(max_pixel_size_for_width(0, 0, 600), 600);
    }

    /// A 1200x800 JPEG whose left half is red and right half blue, tagged with
    /// the given EXIF orientation.
    fn tagged_jpeg(orientation: u16) -> Vec<u8> {
        let img = RgbImage::from_fn(1200, 800, |x, _| {
            if x < 600 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        });
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 90)
            .write_image(img.as_raw(), 1200, 800, image::ExtendedColorType::Rgb8)
            .unwrap();
        // Splice a minimal EXIF APP1 (big-endian TIFF, one IFD0 entry:
        // Orientation) right after SOI.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM\0\x2a\0\0\0\x08");
        tiff.extend_from_slice(&1u16.to_be_bytes());
        tiff.extend_from_slice(&0x0112u16.to_be_bytes());
        tiff.extend_from_slice(&3u16.to_be_bytes());
        tiff.extend_from_slice(&1u32.to_be_bytes());
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0u32.to_be_bytes());
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[test]
    fn landscape_is_resized_to_width() {
        let src = tagged_jpeg(1);
        let jpeg = jpeg_thumbnail(Source::Bytes(&src), 600, 0.8).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (600, 400));
        assert!(decoded.get_pixel(20, 200)[0] > 200, "left stays red");
        assert!(decoded.get_pixel(580, 200)[2] > 200, "right stays blue");
    }

    #[test]
    fn orientation_is_baked_into_pixels() {
        // Orientation 6: display rotated 90° clockwise, so the stored left
        // (red) half ends up on top.
        let src = tagged_jpeg(6);
        let jpeg = jpeg_thumbnail(Source::Bytes(&src), 600, 0.8).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        // Stored 1200x800, displayed 800x1200, limited to 600 wide.
        assert_eq!(decoded.dimensions(), (600, 900));
        assert!(decoded.get_pixel(300, 20)[0] > 200, "top is red");
        assert!(
            decoded.get_pixel(300, decoded.height() - 20)[2] > 200,
            "bottom is blue"
        );
    }

    #[test]
    fn small_sources_are_not_upscaled() {
        let img = RgbImage::from_pixel(320, 200, Rgb([10, 200, 10]));
        let mut src = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut src, 90)
            .write_image(img.as_raw(), 320, 200, image::ExtendedColorType::Rgb8)
            .unwrap();
        let jpeg = jpeg_thumbnail(Source::Bytes(&src), 600, 0.8).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (320, 200));
    }

    #[test]
    fn garbage_is_an_error_not_a_crash() {
        assert!(jpeg_thumbnail(Source::Bytes(b"not an image"), 600, 0.8).is_err());
        assert!(jpeg_thumbnail(Source::File(Path::new("/nonexistent/x.HIF")), 600, 0.8).is_err());
    }
}
