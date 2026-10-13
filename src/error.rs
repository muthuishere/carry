//! One error type for the whole binary; `exit_code()` is the public CLI contract.
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CarryError {
    #[error("{0}")]
    Usage(String),
    #[error("profile '{0}' is read-only")]
    ReadOnly(String),
    #[error("store unreachable: {0}")]
    Unreachable(String),
    #[error("store refused: {0}")]
    Refused(String),
    #[error("bad key: {0}")]
    BadKey(String),
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl CarryError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CarryError::Usage(_) | CarryError::ReadOnly(_) | CarryError::NotFound(_) => 2,
            CarryError::Unreachable(_) => 3,
            CarryError::Refused(_) | CarryError::BadKey(_) | CarryError::Invalid(_) => 4,
            CarryError::Conflict(_) => 5,
            CarryError::Internal(_) => 1,
        }
    }
    /// Stable machine-readable code for JSON output.
    pub fn code(&self) -> &'static str {
        match self {
            CarryError::Usage(_) => "usage",
            CarryError::ReadOnly(_) => "readonly",
            CarryError::Unreachable(_) => "unreachable",
            CarryError::Refused(_) => "refused",
            CarryError::BadKey(_) => "bad_key",
            CarryError::Invalid(_) => "invalid",
            CarryError::NotFound(_) => "not_found",
            CarryError::Conflict(_) => "conflict",
            CarryError::Internal(_) => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, CarryError>;
