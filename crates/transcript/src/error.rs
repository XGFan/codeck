//! Errors of the parsing / assembly layer. codeck-core maps them onto its `CoreError` variant by variant.

pub type TranscriptResult<T> = Result<T, TranscriptError>;

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    /// Carries a short Chinese description of what was not found.
    #[error("未找到{0}")]
    NotFound(String),
    #[error("读取文件失败：{0}")]
    Io(#[from] std::io::Error),
}
