use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub size_bytes: u64,
    pub description: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny",
        name: "Tiny",
        filename: "ggml-tiny.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        size_bytes: 77_691_713,
        description: "Fastest, least accurate",
    },
    ModelInfo {
        id: "base",
        name: "Base",
        filename: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        size_bytes: 147_964_211,
        description: "Good balance for quick tasks",
    },
    ModelInfo {
        id: "small",
        name: "Small",
        filename: "ggml-small.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        size_bytes: 487_626_545,
        description: "Recommended — best accuracy/speed tradeoff",
    },
    ModelInfo {
        id: "medium",
        name: "Medium",
        filename: "ggml-medium.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        size_bytes: 1_533_774_781,
        description: "High accuracy, slower",
    },
    ModelInfo {
        id: "large-v3",
        name: "Large v3",
        filename: "ggml-large-v3.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        size_bytes: 3_094_623_691,
        description: "Best accuracy, slowest",
    },
];

pub fn models_dir() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    let models_dir = data_dir.join("com.romescribe.dev").join("models");
    std::fs::create_dir_all(&models_dir)
        .map_err(|e| format!("Failed to create models directory: {}", e))?;
    Ok(models_dir)
}

pub fn model_path(model_id: &str) -> Result<PathBuf, String> {
    let info = get_model_info(model_id).ok_or(format!("Unknown model: {}", model_id))?;
    Ok(models_dir()?.join(info.filename))
}

pub fn get_model_info(model_id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == model_id)
}

pub fn is_model_downloaded(model_id: &str) -> bool {
    model_path(model_id)
        .map(|p| p.exists())
        .unwrap_or(false)
}
