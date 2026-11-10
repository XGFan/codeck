//! Core error type. Converted to the IPC [`AppError`] at the command boundary.

use crate::model::{AppError, ErrorCode};

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Carries a short Chinese description of what was not found.
    #[error("未找到{0}")]
    NotFound(String),
    /// Carries a Chinese description of why the query is invalid.
    #[error("{0}")]
    InvalidQuery(String),
    #[error("读取文件失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("索引出错：{0}")]
    Index(#[from] rusqlite::Error),
    /// A read-only open found no usable index; carries the reason in Chinese.
    #[error("{0}")]
    IndexUnavailable(String),
    #[error("数据解析失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("操作已取消")]
    Cancelled,
    #[error("{0}")]
    Internal(String),
}

impl CoreError {
    pub fn code(&self) -> ErrorCode {
        match self {
            CoreError::NotFound(_) => ErrorCode::NotFound,
            CoreError::InvalidQuery(_) => ErrorCode::InvalidQuery,
            CoreError::Io(_) => ErrorCode::Io,
            CoreError::Index(_) | CoreError::IndexUnavailable(_) => ErrorCode::Index,
            CoreError::Json(_) | CoreError::Internal(_) => ErrorCode::Internal,
            CoreError::Cancelled => ErrorCode::Cancelled,
        }
    }
}

impl From<transcript::TranscriptError> for CoreError {
    fn from(e: transcript::TranscriptError) -> Self {
        use transcript::TranscriptError as T;
        match e {
            T::NotFound(s) => CoreError::NotFound(s),
            T::Io(e) => CoreError::Io(e),
        }
    }
}

impl From<CoreError> for AppError {
    fn from(e: CoreError) -> Self {
        AppError {
            code: e.code(),
            message: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_app_error_with_code() {
        let e: AppError = CoreError::NotFound("该 Session".into()).into();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert_eq!(e.message, "未找到该 Session");
    }
}
