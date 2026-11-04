//! Errors of the parsing / assembly layer. cv-core maps them onto its `TranscriptError` variant by variant.

pub type TranscriptResult<T> = Result<T, TranscriptError>;

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    /// Carries a short Chinese description of what was not found.
    #[error("未找到{0}")]
    NotFound(String),
    #[error("读取文件失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("数据解析失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Internal(String),
}
