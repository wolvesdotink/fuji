use std::path::Path;

use image::imageops::FilterType;
use image::GenericImageView;
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;

// CLIP ViT-B/32 normalization constants
const CLIP_MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
const CLIP_STD: [f32; 3] = [0.26862954, 0.26130258, 0.27577711];
const IMAGE_SIZE: u32 = 224;

/// Preprocess a JPEG thumbnail for CLIP: resize to 224x224 center crop,
/// convert to RGB float32, normalize with CLIP mean/std.
pub fn preprocess_image(image_path: &str) -> Result<Array4<f32>, String> {
    let img = image::open(Path::new(image_path))
        .map_err(|e| format!("Failed to open image {}: {}", image_path, e))?;

    // Center crop to square
    let (w, h) = img.dimensions();
    let crop_size = w.min(h);
    let x_offset = (w - crop_size) / 2;
    let y_offset = (h - crop_size) / 2;
    let cropped = img.crop_imm(x_offset, y_offset, crop_size, crop_size);

    // Resize to 224x224
    let resized = cropped.resize_exact(IMAGE_SIZE, IMAGE_SIZE, FilterType::Lanczos3);

    // Convert to float32 normalized array [1, 3, 224, 224]
    let mut pixel_data = Array4::<f32>::zeros((1, 3, IMAGE_SIZE as usize, IMAGE_SIZE as usize));

    for y in 0..IMAGE_SIZE {
        for x in 0..IMAGE_SIZE {
            let pixel = resized.get_pixel(x, y);
            for c in 0..3 {
                let val = pixel[c] as f32 / 255.0;
                pixel_data[[0, c, y as usize, x as usize]] =
                    (val - CLIP_MEAN[c]) / CLIP_STD[c];
            }
        }
    }

    Ok(pixel_data)
}

/// Generate a CLIP image embedding from a thumbnail JPEG.
/// Returns a normalized embedding vector.
pub fn embed_image(session: &mut Session, image_path: &str) -> Result<Vec<f32>, String> {
    let pixel_values = preprocess_image(image_path)?;

    // Convert Array4 to (shape, vec) for ort Tensor
    let shape = [1_usize, 3, IMAGE_SIZE as usize, IMAGE_SIZE as usize];
    let data: Vec<f32> = pixel_values.into_raw_vec_and_offset().0;
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
