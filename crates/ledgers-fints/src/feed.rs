use crate::{FintsFeedError, PinSource};
use async_trait::async_trait;
use chrono::{Local, NaiveDate};
use fints::{Flow, Pin, ProductId, TransactionStatus, UserId};
use hex::ToHex;
use ledgers_commands::CommandError;
use ledgers_ext::scope::CompanyId;
use ledgers_ext::{BankFeed, BankTransaction};
use ledgers_money::{Amount, Currency};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;
use tracing::debug;

#[derive(Clone, Debug)]
pub enum BankIdentifier {
    Blz(String),
    Url(String),
}

#[derive(Clone, Debug)]
pub struct FintsFeedConfig {
    pub bank: BankIdentifier,
    pub user_id: String,
    pub product_id_source: ProductIdSource,
    pub iban: String,
    pub bic: Option<String>,
    pub system_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ProductIdSource {
    EnvOrFail,
    Explicit(String),
}

impl ProductIdSource {
    /// Resolve the product ID from the configured source.
    ///
    /// # Errors
    ///
    /// Returns `ProductIdRequired` if `EnvOrFail` is set and the environment variable is not set.
    pub fn resolve(&self) -> Result<String, FintsFeedError> {
        match self {
            ProductIdSource::EnvOrFail => option_env!("LEDGERS_FINTS_PRODUCT_ID")
                .map(std::string::ToString::to_string)
                .ok_or(FintsFeedError::ProductIdRequired),
            ProductIdSource::Explicit(id) => Ok(id.clone()),
        }
    }
}

impl FintsFeedConfig {
    pub fn new_with_blz(
        blz: impl Into<String>,
        user_id: impl Into<String>,
        iban: impl Into<String>,
        bic: Option<String>,
    ) -> Self {
        FintsFeedConfig {
            bank: BankIdentifier::Blz(blz.into()),
            user_id: user_id.into(),
            product_id_source: ProductIdSource::EnvOrFail,
            iban: iban.into(),
            bic,
            system_id: None,
        }
    }

    #[must_use]
    pub fn with_product_id(mut self, product_id: impl Into<String>) -> Self {
        self.product_id_source = ProductIdSource::Explicit(product_id.into());
        self
    }

    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.bank = BankIdentifier::Url(url.into());
        self
    }

    #[must_use]
    pub fn with_system_id(mut self, system_id: impl Into<String>) -> Self {
        self.system_id = Some(system_id.into());
        self
    }

    /// Validate the feed configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid (empty [`user_id`], [`iban`], or invalid IBAN format).
    pub fn validate(&self) -> Result<(), FintsFeedError> {
        if self.user_id.is_empty() {
            return Err(FintsFeedError::InvalidConfig("user_id is empty".into()));
        }
        if self.iban.is_empty() {
            return Err(FintsFeedError::InvalidConfig("iban is empty".into()));
        }
        if self.iban.len() < 15
            || self.iban.len() > 34
            || !self.iban.chars().all(char::is_alphanumeric)
        {
            return Err(FintsFeedError::InvalidAccount(format!(
                "invalid IBAN: {}",
                self.iban
            )));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AccountBalance {
    pub amount: Amount,
    pub date: String,
}

pub struct FintsFeed {
    config: FintsFeedConfig,
    pin_source: Arc<dyn PinSource>,
}

impl FintsFeed {
    /// Create a new `FinTS` feed with the given configuration and PIN source.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration validation fails.
    pub fn new(
        config: FintsFeedConfig,
        pin_source: Arc<dyn PinSource>,
    ) -> Result<Self, FintsFeedError> {
        config.validate()?;
        Ok(FintsFeed { config, pin_source })
    }

    /// Get the current account balance.
    ///
    /// # Errors
    ///
    /// Returns an error if the `FinTS` dialog fails or the balance cannot be retrieved.
    pub async fn balance(&self) -> Result<AccountBalance, CommandError> {
        self.config
            .validate()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let pin = self
            .pin_source
            .get_pin(&self.config.iban)
            .await
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let product_id = self
            .config
            .product_id_source
            .resolve()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let blz = match &self.config.bank {
            BankIdentifier::Blz(b) => b.clone(),
            BankIdentifier::Url(_) => {
                return Err(CommandError::InvalidInput(
                    "custom FinTS URLs not yet supported".into(),
                ));
            }
        };

        let user_id = UserId::new(&self.config.user_id);
        let pin_obj = Pin::new(pin.expose());
        let product_id_obj = ProductId::new(&product_id);

        let (mut flow, _challenge) =
            Flow::initiate(&blz, &user_id, &pin_obj, &product_id_obj, None, None, None)
                .await
                .map_err(|e: fints::error::FinTSError| {
                    let fe: FintsFeedError = e.into();
                    CommandError::InvalidInput(fe.to_string())
                })?;

        let bic = self.config.bic.as_deref().unwrap_or("UNKNOWN");
        let result = flow
            .confirm_and_fetch(&self.config.iban, bic, 0)
            .await
            .map_err(|e| CommandError::InvalidInput(FintsFeedError::from(e).to_string()))?;

        let bal = result
            .balance
            .ok_or_else(|| CommandError::InvalidInput("no balance".into()))?;

        let currency = Currency::from_code(bal.currency.as_ref())
            .map_err(|e| CommandError::InvalidInput(format!("unknown currency: {e}")))?;
        let amount = Amount::parse_canonical(bal.amount.to_string().as_str(), currency)?;

        Ok(AccountBalance {
            amount,
            date: bal.date.to_string(),
        })
    }
}

impl fmt::Debug for FintsFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FintsFeed")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[async_trait(?Send)]
impl BankFeed for FintsFeed {
    async fn pull(
        &self,
        _company: &CompanyId,
        account_id: &str,
        since: &str,
    ) -> Result<Vec<BankTransaction>, CommandError> {
        if account_id != self.config.iban {
            return Err(CommandError::InvalidInput(format!(
                "account_id {account_id} does not match configured IBAN"
            )));
        }

        let since_date = NaiveDate::parse_from_str(since, "%Y-%m-%d")
            .map_err(|_| CommandError::InvalidInput(format!("invalid date: {since}")))?;

        self.config
            .validate()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let pin = self
            .pin_source
            .get_pin(&self.config.iban)
            .await
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let product_id = self
            .config
            .product_id_source
            .resolve()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;

        let blz = match &self.config.bank {
            BankIdentifier::Blz(b) => b.clone(),
            BankIdentifier::Url(_) => {
                return Err(CommandError::InvalidInput(
                    "custom FinTS URLs not yet supported".into(),
                ));
            }
        };

        let user_id = UserId::new(&self.config.user_id);
        let pin_obj = Pin::new(pin.expose());
        let product_id_obj = ProductId::new(&product_id);

        debug!("Initiating FinTS dialog for {}", account_id);

        let (mut flow, _challenge) =
            Flow::initiate(&blz, &user_id, &pin_obj, &product_id_obj, None, None, None)
                .await
                .map_err(|e| CommandError::InvalidInput(FintsFeedError::from(e).to_string()))?;

        let bic = self.config.bic.as_deref().unwrap_or("UNKNOWN");
        let today = Local::now().naive_local().date();
        let days = u32::try_from((today - since_date).num_days()).unwrap_or(0) + 1;

        let result = flow
            .confirm_and_fetch(&self.config.iban, bic, days)
            .await
            .map_err(|e| CommandError::InvalidInput(FintsFeedError::from(e).to_string()))?;

        let mut txs = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        for tx in result.transactions {
            if tx.status != TransactionStatus::Booked {
                continue;
            }

            let currency = Currency::from_code(tx.currency.as_ref())
                .map_err(|e| CommandError::InvalidInput(format!("unknown currency: {e}")))?;
            let amount = Amount::parse_canonical(tx.amount.to_string().as_str(), currency)?;

            let booking_date = tx.date.to_string();
            let valuta_date = tx.valuta_date.map(|d| d.to_string());

            let counterparty = tx
                .applicant_name
                .clone()
                .or_else(|| {
                    tx.applicant_iban
                        .as_ref()
                        .map(std::string::ToString::to_string)
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let reference = tx
                .purpose
                .clone()
                .or_else(|| tx.reference.clone())
                .unwrap_or_default();

            let counterparty_iban_str = tx
                .applicant_iban
                .as_ref()
                .map(std::string::ToString::to_string);
            let counterparty_iban_opt = counterparty_iban_str.as_deref();
            let external_id = generate_external_id(
                &self.config.iban,
                &booking_date,
                valuta_date.as_deref(),
                tx.amount.to_string().as_str(),
                tx.currency.as_ref(),
                counterparty_iban_opt,
                &reference,
                &seen_ids,
            );

            seen_ids.insert(external_id.clone());

            txs.push(BankTransaction {
                booking_date,
                counterparty,
                reference,
                external_id: Some(external_id),
                amount,
            });
        }

        debug!("Imported {} transactions", txs.len());
        Ok(txs)
    }
}

/// Generate a stable external ID for a bank transaction.
///
/// # Must Use
///
/// This function generates deterministic IDs with collision detection.
#[must_use]
#[allow(clippy::too_many_arguments, clippy::implicit_hasher)]
pub fn generate_external_id(
    // exposed for tests
    iban: &str,
    booking_date: &str,
    valuta_date: Option<&str>,
    amount: &str,
    currency: &str,
    counterparty_iban: Option<&str>,
    purpose: &str,
    seen_ids: &std::collections::HashSet<String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(iban.as_bytes());
    hasher.update(booking_date.as_bytes());
    if let Some(vd) = valuta_date {
        hasher.update(vd.as_bytes());
    }
    hasher.update(amount.as_bytes());
    hasher.update(currency.as_bytes());
    if let Some(ci) = counterparty_iban {
        hasher.update(ci.as_bytes());
    }
    hasher.update(purpose.as_bytes());

    let hash = hasher.finalize();
    let hash_hex = hash.encode_hex::<String>();

    let mut occurrence = 0;
    loop {
        let candidate = format!("fints:{hash_hex}:{occurrence}");
        if !seen_ids.contains(&candidate) {
            return candidate;
        }
        occurrence += 1;
    }
}
