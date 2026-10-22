use ledgers_commands::CommandError;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum FintsFeedError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("authentication failed")]
    AuthenticationFailed,
    #[error("operation cancelled")]
    Cancelled,
    #[error("PIN source not available")]
    PinSourceRequired,
    #[error("product ID required: set LEDGERS_FINTS_PRODUCT_ID or configure explicitly")]
    ProductIdRequired,
    #[error("invalid account: {0}")]
    InvalidAccount(String),
    #[error("bank error: {0}")]
    BankError(String),
}

impl From<FintsFeedError> for CommandError {
    fn from(err: FintsFeedError) -> Self {
        CommandError::InvalidInput(err.to_string())
    }
}

impl From<fints::error::FinTSError> for FintsFeedError {
    fn from(err: fints::error::FinTSError) -> Self {
        let msg = err.to_string();
        if msg.to_lowercase().contains("authentication")
            || msg.to_lowercase().contains("pin")
            || msg.to_lowercase().contains("tan")
        {
            FintsFeedError::AuthenticationFailed
        } else {
            FintsFeedError::BankError(msg)
        }
    }
}
