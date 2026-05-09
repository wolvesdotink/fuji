use std::fs;
use std::io::Write;
use std::path::Path;
use futures_util::StreamExt;
use tauri::ipc::Channel;

use crate::models::ModelDownloadProgress;

const VISION_MODEL_URL: &str = "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model_quantized.onnx";
const TEXT_MODEL_URL: &str = "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/text_model_quantized.onnx";
const TOKENIZER_URL: &str = "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/tokenizer.json";

pub fn vision_model_path(model_dir: &str) -> String {
    Path::new(model_dir)
        .join("vision_model_quantized.onnx")
        .to_string_lossy()
        .to_string()
}

pub fn text_model_path(model_dir: &str) -> String {
    Path::new(model_dir)
        .join("text_model_quantized.onnx")
        .to_string_lossy()
        .to_string()
}

pub fn tokenizer_path(model_dir: &str) -> String {
    Path::new(model_dir)
        .join("tokenizer.json")
        .to_string_lossy()
        .to_string()
}

pub fn models_exist(model_dir: &str) -> bool {
    Path::new(&vision_model_path(model_dir)).exists()
        && Path::new(&text_model_path(model_dir)).exists()
        && Path::new(&tokenizer_path(model_dir)).exists()
}

async fn download_file(
    url: &str,
    dest_path: &str,
    file_name: &str,
    on_progress: &Channel<ModelDownloadProgress>,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to download {}: {}", file_name, e))?;

    let total_size = response.content_length().unwrap_or(0);

    let dest = Path::new(dest_path);
    let mut file = fs::File::create(dest)
        .map_err(|e| format!("Failed to create {}: {}", dest_path, e))?;

    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download error for {}: {}", file_name, e))?;
        file.write_all(&chunk)
            .map_err(|e| format!("Write error for {}: {}", file_name, e))?;

        downloaded += chunk.len() as u64;

        let _ = on_progress.send(ModelDownloadProgress {
            bytes_downloaded: downloaded,
            bytes_total: total_size,
            file_name: file_name.to_string(),
        });
    }

    Ok(())
}

pub async fn ensure_models(
    model_dir: &str,
    on_progress: Channel<ModelDownloadProgress>,
) -> Result<(), String> {
    if models_exist(model_dir) {
        return Ok(());
    }

    let dir = Path::new(model_dir);
    fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create model directory: {}", e))?;

    // Download vision model
    let vision_path = vision_model_path(model_dir);
    if !Path::new(&vision_path).exists() {
        log::info!("Downloading CLIP vision model...");
        download_file(
            VISION_MODEL_URL,
            &vision_path,
            "vision_model_quantized.onnx",
            &on_progress,
        )
        .await?;
    }

    // Download text model
    let text_path = text_model_path(model_dir);
    if !Path::new(&text_path).exists() {
        log::info!("Downloading CLIP text model...");
        download_file(
            TEXT_MODEL_URL,
            &text_path,
            "text_model_quantized.onnx",
            &on_progress,
        )
        .await?;
    }

    // Download tokenizer
    let tok_path = tokenizer_path(model_dir);
    if !Path::new(&tok_path).exists() {
        log::info!("Downloading CLIP tokenizer...");
        download_file(
            TOKENIZER_URL,
            &tok_path,
            "tokenizer.json",
            &on_progress,
        )
        .await?;
    }

    log::info!("All CLIP models downloaded successfully.");
    Ok(())
}
