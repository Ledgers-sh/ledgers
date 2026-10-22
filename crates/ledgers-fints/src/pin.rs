use crate::FintsFeedError;
use async_trait::async_trait;
use std::fmt;
use zeroize::Zeroize;

#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct SecurePin(String);

impl SecurePin {
    #[must_use]
    pub fn new(pin: String) -> Self {
        SecurePin(pin)
    }
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecurePin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("****")
    }
}

impl fmt::Display for SecurePin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("****")
    }
}

#[async_trait(?Send)]
pub trait PinSource: Send + Sync {
    async fn get_pin(&self, account_id: &str) -> Result<SecurePin, FintsFeedError>;
}

type PromptFn = dyn Fn(&str) -> Option<String> + Send + Sync;

#[derive(Clone)]
pub struct DefaultPinSource {
    prompt: std::sync::Arc<PromptFn>,
}

impl DefaultPinSource {
    pub fn new<F>(prompt: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        DefaultPinSource {
            prompt: std::sync::Arc::new(prompt),
        }
    }
}

impl fmt::Debug for DefaultPinSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultPinSource").finish()
    }
}

#[async_trait(?Send)]
impl PinSource for DefaultPinSource {
    async fn get_pin(&self, account_id: &str) -> Result<SecurePin, FintsFeedError> {
        match (self.prompt)(account_id) {
            Some(pin) => Ok(SecurePin::new(pin)),
            None => Err(FintsFeedError::Cancelled),
        }
    }
}

#[cfg(feature = "keychain")]
pub struct KeychainPinSource {
    service: String,
}

#[cfg(feature = "keychain")]
impl KeychainPinSource {
    pub fn new(service: impl Into<String>) -> Self {
        KeychainPinSource {
            service: service.into(),
        }
    }

    pub async fn save_pin(&self, account_id: &str, pin: &SecurePin) -> Result<(), FintsFeedError> {
        let entry = keyring::Entry::new(&self.service, account_id)
            .map_err(|e| FintsFeedError::InvalidConfig(format!("keyring: {e}")))?;
        entry
            .set_password(pin.expose())
            .map_err(|e| FintsFeedError::InvalidConfig(format!("keyring: {e}")))
    }
}

#[cfg(feature = "keychain")]
impl fmt::Debug for KeychainPinSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeychainPinSource")
            .field("service", &self.service)
            .finish()
    }
}

#[cfg(feature = "keychain")]
#[async_trait(?Send)]
impl PinSource for KeychainPinSource {
    async fn get_pin(&self, account_id: &str) -> Result<SecurePin, FintsFeedError> {
        let entry = keyring::Entry::new(&self.service, account_id)
            .map_err(|e| FintsFeedError::InvalidConfig(format!("keyring: {e}")))?;
        match entry.get_password() {
            Ok(pin) => Ok(SecurePin::new(pin)),
            Err(keyring::Error::NoEntry) => Err(FintsFeedError::PinSourceRequired),
            Err(e) => Err(FintsFeedError::InvalidConfig(format!("keyring: {e}"))),
        }
    }
}
