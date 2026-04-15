use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin";
const MODEL_FILENAME: &str = "ggml-large-v3-turbo.bin";

#[derive(Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: f32,
}

pub fn model_path(models_dir: &Path) -> PathBuf {
    models_dir.join(MODEL_FILENAME)
}

pub fn is_model_downloaded(models_dir: &Path) -> bool {
    model_path(models_dir).exists()
}

pub async fn download_model(models_dir: &Path, app: &AppHandle) -> Result<()> {
    std::fs::create_dir_all(models_dir)?;
    let dest = model_path(models_dir);

    let client = reqwest::Client::new();
    let response = client
        .get(MODEL_URL)
        .send()
        .await
        .context("Kunne ikke downloade Whisper-model")?;

    let total_size = response.content_length();
    let mut downloaded: u64 = 0;
    let mut file = tokio::fs::File::create(&dest).await?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Fejl under download")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        downloaded += chunk.len() as u64;

        let percent = total_size
            .map(|total| (downloaded as f32 / total as f32) * 100.0)
            .unwrap_or(0.0);

        let _ = app.emit(
            "model-download-progress",
            DownloadProgress {
                downloaded,
                total: total_size,
                percent,
            },
        );
    }

    Ok(())
}
