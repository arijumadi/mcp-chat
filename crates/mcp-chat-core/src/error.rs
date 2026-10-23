use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Message(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("cancelled")]
    Cancelled,

    #[error("the model's context window was exceeded")]
    ContextOverflow,

    #[error("the response was still truncated after multiple continuations")]
    OutputLimit,

    #[error("authentication failed")]
    Unauthorized,
}

impl CoreError {
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }

    pub fn invalid(text: impl Into<String>) -> Self {
        Self::InvalidInput(text.into())
    }

    /// Human-readable text safe to show in the transcript. Never include raw
    /// response bodies or headers here.
    pub fn user_message(&self) -> String {
        match self {
            Self::Db(_) => "The local database could not be updated.".into(),
            Self::Http(err) if err.is_timeout() => {
                "The request timed out. Check the endpoint and try again.".into()
            }
            Self::Http(err) if err.is_connect() => {
                "Could not connect to the endpoint. Check the URL and your network.".into()
            }
            Self::Http(_) => "The network request failed.".into(),
            Self::Json(_) => "The server returned a response this app could not read.".into(),
            Self::Io(_) => "A local file operation failed.".into(),
            Self::Message(text) | Self::InvalidInput(text) | Self::NotFound(text) => text.clone(),
            Self::Cancelled => "Stopped.".into(),
            Self::ContextOverflow => {
                "This conversation is too long for the selected model, even after compaction."
                    .into()
            }
            Self::OutputLimit => {
                "The model kept hitting its output limit. Try asking for a smaller piece of work."
                    .into()
            }
            Self::Unauthorized => "Authentication failed. Check the API key.".into(),
        }
    }
}

pub fn is_context_overflow_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("context window")
        || lower.contains("prompt is too long")
        || lower.contains("maximum context length")
        || lower.contains("input is too long")
}

pub fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}
