use std::path::Path;

use image::imageops::FilterType;
use image::GenericImageView;
use image::RgbImage;
use ort::session::Session;
use ort::value::Tensor;

// CLIP ViT-B/32 normalization constants
const CLIP_MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
const CLIP_STD: [f32; 3] = [0.26862954, 0.26130258, 0.27577711];
const IMAGE_SIZE: u32 = 224;

/// Pack a 224x224 RGB image into a flat CHW float32 buffer, normalized with the
/// CLIP mean/std. Layout matches an ndarray `[1, 3, H, W]` in C-order: the flat
/// index is `c * H * W + y * W + x`. Iterating the raw `RgbImage` buffer (HWC,
/// tightly packed as r,g,b,r,g,b,…) avoids the per-pixel bounds checks and trait
/// dispatch of `get_pixel`.
fn pack_chw(rgb: &RgbImage) -> Vec<f32> {
    let (w, h) = rgb.dimensions();
    let plane = (w as usize) * (h as usize);
    let mut data = vec![0f32; 3 * plane];

    for (i, px) in rgb.as_raw().chunks_exact(3).enumerate() {
        for c in 0..3 {
            let val = px[c] as f32 / 255.0;
            data[c * plane + i] = (val - CLIP_MEAN[c]) / CLIP_STD[c];
        }
    }

    data
}

/// Preprocess a JPEG thumbnail for CLIP: center-crop to square, resize to
/// 224x224, convert to RGB float32, normalize with CLIP mean/std. Returns a flat
/// CHW `[1, 3, 224, 224]` buffer ready to hand to an ort `Tensor`.
pub fn preprocess_image(image_path: &str) -> Result<Vec<f32>, String> {
    let img = image::open(Path::new(image_path))
        .map_err(|e| format!("Failed to open image {}: {}", image_path, e))?;

    // Center crop to square
    let (w, h) = img.dimensions();
    let crop_size = w.min(h);
    let x_offset = (w - crop_size) / 2;
    let y_offset = (h - crop_size) / 2;
    let cropped = img.crop_imm(x_offset, y_offset, crop_size, crop_size);

    // Resize to 224x224. Triangle (bilinear) is markedly cheaper than Lanczos3
    // and, at this thumbnail scale, produces embeddings that are effectively
    // indistinguishable for retrieval.
    let resized = cropped.resize_exact(IMAGE_SIZE, IMAGE_SIZE, FilterType::Triangle);

    Ok(pack_chw(&resized.to_rgb8()))
}

/// Generate a CLIP image embedding from a thumbnail JPEG.
/// Returns a normalized embedding vector.
pub fn embed_image(session: &mut Session, image_path: &str) -> Result<Vec<f32>, String> {
    let data = preprocess_image(image_path)?;

    let shape = [1_usize, 3, IMAGE_SIZE as usize, IMAGE_SIZE as usize];
    let input_tensor = Tensor::from_array((shape, data))
        .map_err(|e| format!("Failed to create input tensor: {}", e))?;

    let outputs = session
        .run(ort::inputs!["pixel_values" => input_tensor])
        .map_err(|e| format!("CLIP vision inference failed: {}", e))?;

    extract_embedding(&outputs[0])
}

/// Extract embedding from an ort output Value.
/// Handles both [1, hidden_dim] and [1, seq_len, hidden_dim] shapes.
pub fn extract_embedding(output: &ort::value::Value) -> Result<Vec<f32>, String> {
    // try_extract_tensor returns (&Shape, &[f32]) — shape info + flat data slice
    let extracted = output
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("Failed to extract output tensor: {}", e))?;

    // Get shape from the Value's shape method, and data from extracted flat slice
    let raw_data: &[f32] = extracted.1;
    let shape: Vec<i64> = extracted.0.iter().copied().collect();
    let shape: Vec<usize> = shape.iter().map(|&d| d as usize).collect();

    let embedding: Vec<f32> = if shape.len() == 3 {
        // [1, seq_len, hidden_dim] — take the first token (CLS)
        let hidden_dim = shape[2];
        raw_data[..hidden_dim].to_vec()
    } else if shape.len() == 2 {
        // [1, hidden_dim] — pooler output
        let hidden_dim = shape[1];
        raw_data[..hidden_dim].to_vec()
    } else {
        return Err(format!("Unexpected output shape: {:?}", shape));
    };

    Ok(l2_normalize(&embedding))
}

pub fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-12 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// The raw-buffer CHW packer must produce byte-for-byte the same layout and
    /// values as the previous per-pixel `get_pixel` approach: a `[1, 3, H, W]`
    /// C-order tensor with `data[c * H * W + y * W + x] = (v/255 - mean)/std`.
    #[test]
    fn pack_chw_matches_get_pixel_reference() {
        let (w, h) = (5u32, 3u32);
        // Deterministic, distinct-per-channel pixel values so a transposed or
        // mis-strided layout would fail the comparison.
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let base = (y * w + x) as u8;
                img.put_pixel(
                    x,
                    y,
                    Rgb([base, base.wrapping_add(50), base.wrapping_add(100)]),
                );
            }
        }

        let packed = pack_chw(&img);

        let plane = (w * h) as usize;
        assert_eq!(packed.len(), 3 * plane);

        // Reference: the exact loop the old preprocess_image ran.
        for y in 0..h {
            for x in 0..w {
                let pixel = img.get_pixel(x, y);
                for c in 0..3 {
                    let val = pixel[c] as f32 / 255.0;
                    let expected = (val - CLIP_MEAN[c]) / CLIP_STD[c];
                    let idx = c * plane + (y * w + x) as usize;
                    assert_eq!(packed[idx], expected, "mismatch at c={c} y={y} x={x}");
                }
            }
        }
    }
}
