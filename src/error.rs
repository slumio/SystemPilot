use std::fmt;
use std::path::PathBuf;

/// Errors returned at SysPilot's filesystem, configuration, and protocol
/// boundaries. Keeping these typed makes CLI failures actionable without
/// leaking panics or silently falling back to an unexpected configuration.
#[derive(Debug)]
pub enum AppError {
    Io {
        context: &'static str,
        source: std::io::Error,
    },
    ConfigParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    Validation(String),
    Protocol(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{}: {}", context, source),
            Self::ConfigParse { path, source } => {
                write!(f, "invalid configuration at {}: {}", path.display(), source)
            }
            Self::Validation(message) | Self::Protocol(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::ConfigParse { source, .. } => Some(source),
            Self::Validation(_) | Self::Protocol(_) => None,
        }
    }
}
