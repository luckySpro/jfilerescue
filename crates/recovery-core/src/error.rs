use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("访问被拒绝: {0}")]
    AccessDenied(String),
    #[error("无法识别的文件系统")]
    UnknownFilesystem,
    #[error("不支持的文件系统: {0}")]
    Unsupported(String),
    #[error("数据损坏: {0}")]
    Corrupt(String),
    #[error("无效输入: {0}")]
    InvalidInput(String),
    #[error("目标与源在同一磁盘上，已拒绝写入")]
    SameDisk,
    #[error("目标空间不足: 需要 {needed} 字节, 可用 {available} 字节")]
    NoSpace { needed: u64, available: u64 },
    #[error("操作已取消")]
    Cancelled,
    #[error("设备正忙: {0}")]
    Busy(String),
    #[error("会话错误: {0}")]
    Session(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// A short, stable code the UI maps to localized, actionable text.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied => "access_denied",
            Error::Io(_) => "io",
            Error::AccessDenied(_) => "access_denied",
            Error::UnknownFilesystem => "unknown_fs",
            Error::Unsupported(_) => "unsupported",
            Error::Corrupt(_) => "corrupt",
            Error::InvalidInput(_) => "invalid_input",
            Error::SameDisk => "same_disk",
            Error::NoSpace { .. } => "no_space",
            Error::Cancelled => "cancelled",
            Error::Busy(_) => "busy",
            Error::Session(_) => "session",
            Error::Other(_) => "other",
        }
    }
}
