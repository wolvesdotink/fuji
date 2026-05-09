use byteorder::{BigEndian, ReadBytesExt};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

/// Extract the embedded JPEG preview from a Fuji RAF file.
///
/// RAF format header layout:
/// - Bytes 0..16: Magic string "FUJIFILMCCD-RAW "
/// - Bytes 84..88: JPEG image offset (u32 big-endian)
/// - Bytes 88..92: JPEG image length (u32 big-endian)
pub fn extract_jpeg_from_raf(raf_path: &Path) -> Result<Vec<u8>, String> {
    let mut file = File::open(raf_path).map_err(|e| format!("Failed to open RAF: {}", e))?;

    // Verify magic bytes
    let mut magic = [0u8; 16];
    file.read_exact(&mut magic)
        .map_err(|e| format!("Failed to read RAF magic: {}", e))?;

    let magic_str = String::from_utf8_lossy(&magic);
    if !magic_str.starts_with("FUJIFILMCCD-RAW") {
        return Err(format!("Not a valid RAF file: magic = {:?}", magic_str));
    }

    // Read JPEG offset and length at bytes 84-92
    file.seek(SeekFrom::Start(84))
        .map_err(|e| format!("Failed to seek to JPEG offset: {}", e))?;

    let jpeg_offset = file
        .read_u32::<BigEndian>()
        .map_err(|e| format!("Failed to read JPEG offset: {}", e))?;

    let jpeg_length = file
        .read_u32::<BigEndian>()
        .map_err(|e| format!("Failed to read JPEG length: {}", e))?;

    if jpeg_length == 0 || jpeg_offset == 0 {
        return Err("RAF file has no embedded JPEG preview".to_string());
    }

    // Seek to the JPEG data and read it
    file.seek(SeekFrom::Start(jpeg_offset as u64))
        .map_err(|e| format!("Failed to seek to JPEG data: {}", e))?;

    let mut jpeg_data = vec![0u8; jpeg_length as usize];
    file.read_exact(&mut jpeg_data)
        .map_err(|e| format!("Failed to read JPEG data: {}", e))?;

    Ok(jpeg_data)
}

/// Extract and resize the embedded JPEG to a thumbnail.
/// Returns WebP bytes of the resized thumbnail.
///
/// Applies EXIF orientation before resize/encode: Fuji cameras capture sensor
/// data in landscape and mark portrait shots with an orientation tag. WebP
/// encoding strips EXIF, so we must rotate pixels in-place — otherwise every
/// portrait shot renders sideways in the gallery.
pub fn extract_thumbnail(raf_path: &Path, max_width: u32) -> Result<Vec<u8>, String> {
    let jpeg_data = extract_jpeg_from_raf(raf_path)?;

    // Build a decoder so we can read the EXIF orientation tag before decoding.
    let reader = ImageReader::new(Cursor::new(&jpeg_data))
        .with_guessed_format()
        .map_err(|e| format!("Failed to guess image format: {}", e))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| format!("Failed to build decoder: {}", e))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)
        .map_err(|e| format!("Failed to decode embedded JPEG: {}", e))?;
    img.apply_orientation(orientation);

    // Only resize if wider than max_width (post-rotation dimensions).
    let resized = if img.width() > max_width {
        let ratio = max_width as f64 / img.width() as f64;
        let new_height = (img.height() as f64 * ratio) as u32;
        img.resize(max_width, new_height, FilterType::Triangle)
    } else {
        img
    };

    // Encode to WebP
    let mut output = Cursor::new(Vec::new());
    resized
        .write_to(&mut output, image::ImageFormat::WebP)
        .map_err(|e| format!("Failed to encode thumbnail: {}", e))?;

    Ok(output.into_inner())
}
