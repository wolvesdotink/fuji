use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use super::image::extract_embedding;

const MAX_SEQ_LEN: usize = 77;

/// Tokenize a text query for CLIP.
/// Pads/truncates to 77 tokens with start/end tokens.
pub fn tokenize_text(tokenizer: &Tokenizer, text: &str) -> Result<Vec<i64>, String> {
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|e| format!("Tokenization failed: {}", e))?;

    let mut input_ids = vec![0i64; MAX_SEQ_LEN];

    let ids = encoding.get_ids();
    let len = ids.len().min(MAX_SEQ_LEN);

    for i in 0..len {
        input_ids[i] = ids[i] as i64;
    }

    Ok(input_ids)
}

/// Generate a CLIP text embedding from a query string.
/// Returns a normalized embedding vector.
pub fn embed_text(
    session: &mut Session,
    tokenizer: &Tokenizer,
    text: &str,
) -> Result<Vec<f32>, String> {
    let input_ids = tokenize_text(tokenizer, text)?;

    let shape: [usize; 2] = [1, MAX_SEQ_LEN];

    let ids_tensor = Tensor::from_array((shape, input_ids))
        .map_err(|e| format!("Failed to create input_ids tensor: {}", e))?;

    let outputs = session
        .run(ort::inputs!["input_ids" => ids_tensor])
        .map_err(|e| format!("CLIP text inference failed: {}", e))?;

    extract_embedding(&outputs[0])
}
