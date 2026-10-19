//! The account domain model: one node of a company's chart of accounts.
//!
//! Everything here is pure data plus validation — no I/O. Storage lives in
//! [`crate::store`], commands in [`crate::commands`].

use crate::error::AccountsError;
use crate::i18n::{self, Locale};
use ledgers_money::Currency;
use serde::{Deserialize, Serialize};

/// The maximum number of tags one account may carry.
pub const MAX_TAGS: usize = 16;

/// The five classical account classes, stored as lowercase strings
/// (`"asset"`, `"liability"`, `"equity"`, `"income"`, `"expense"`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    /// Resources the company controls (cash, receivables, plant).
    Asset,
    /// Obligations the company owes (payables, tax due).
    Liability,
    /// The owners' stake, including retained earnings.
    Equity,
    /// Revenue earned (credit normal).
    Income,
    /// Costs incurred (debit normal).
    Expense,
}

impl AccountType {
    /// The balance side that increases accounts of this type:
    /// asset/expense → [`NormalBalance::Debit`],
    /// liability/equity/income → [`NormalBalance::Credit`].
    ///
    /// This is only the *default* — a contra account stores its own
    /// `normal_balance` (e.g. accumulated depreciation is an asset with a
    /// credit normal balance).
    #[must_use]
    pub const fn default_normal_balance(self) -> NormalBalance {
        match self {
            Self::Asset | Self::Expense => NormalBalance::Debit,
            Self::Liability | Self::Equity | Self::Income => NormalBalance::Credit,
        }
    }
}

impl std::fmt::Display for AccountType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Asset => "asset",
            Self::Liability => "liability",
            Self::Equity => "equity",
            Self::Income => "income",
            Self::Expense => "expense",
        })
    }
}

impl std::str::FromStr for AccountType {
    type Err = AccountsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "asset" => Ok(Self::Asset),
            "liability" => Ok(Self::Liability),
            "equity" => Ok(Self::Equity),
            "income" => Ok(Self::Income),
            "expense" => Ok(Self::Expense),
            _ => Err(AccountsError::UnknownAccountType {
                value: s.to_owned(),
            }),
        }
    }
}

/// The balance side that increases an account: `"debit"` or `"credit"`.
///
/// Stored per account (not derived from the type) because contra accounts
/// exist: accumulated depreciation is an [`AccountType::Asset`] with a
/// credit normal balance, sales returns an income account with a debit one.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NormalBalance {
    /// Debit-increases.
    Debit,
    /// Credit-increases.
    Credit,
}

impl std::fmt::Display for NormalBalance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Debit => "debit",
            Self::Credit => "credit",
        })
    }
}

impl std::str::FromStr for NormalBalance {
    type Err = AccountsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "debit" => Ok(Self::Debit),
            "credit" => Ok(Self::Credit),
            _ => Err(AccountsError::UnknownNormalBalance {
                value: s.to_owned(),
            }),
        }
    }
}

/// One account of one company's chart: a node in the account tree.
///
/// `postable` vs `heading` is **derived, never stored**: an account is
/// postable exactly when it has no children (a leaf); an account with
/// children is a heading. Deriving it keeps the flag truthful by
/// construction — adding a child to a leaf makes it a heading atomically,
/// in the same INSERT, whereas a stored flag is a second write that can
/// go stale. A stale flag is unsafe for agents: an agent that reads a
/// heading as postable drafts entries against an account that must never
/// hold a balance. See [`AccountEntry`] for the read-side wrapper that
/// carries the derived state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Account {
    /// The company this account belongs to (the storage crate's `company`
    /// anchor; no cross-module foreign key — see the migration comments).
    pub company_id: String,
    /// The account number: 2..=10 ASCII digits, a sortable string. Charts
    /// use one fixed width so lexical order is numeric order.
    pub number: String,
    /// The translatable name key (`"account.generic_sme.cash"`); must
    /// start with `"account."`. Translated per locale at read time —
    /// document text is data, never instructions (docs/PLAN.md §3 rule 8).
    pub name_key: String,
    /// The account class.
    pub account_type: AccountType,
    /// The balance side that increases this account — an own field, not a
    /// type derivation, so contra accounts (asset with credit normal,
    /// e.g. accumulated depreciation) are representable.
    pub normal_balance: NormalBalance,
    /// The account's own currency; `None` means the company base currency.
    /// `Currency` values only exist for table entries of `ledgers-money`,
    /// so a currency here has already passed `Currency::from_code`.
    pub currency: Option<Currency>,
    /// The parent account number; `None` = root of the chart.
    pub parent: Option<String>,
    /// Free-form kebab-case tags (`"bank"`, `"ar"`, `"ap"`, `"vat-input"`,
    /// `"vat-output"`, `"fx-gain"`, `"fx-loss"`, `"rounding"`,
    /// `"suspense"`, `"retained-earnings"`). Some tags are load-bearing:
    /// the five system tags name the accounts posting machinery needs.
    pub tags: Vec<String>,
    /// Archived accounts stay in the chart for history but accept no new
    /// postings; accounts are never deleted (docs/PLAN.md §3 rule 3).
    pub archived: bool,
    /// ISO 8601 UTC creation timestamp.
    pub created_at: String,
    /// ISO 8601 UTC last-update timestamp.
    pub updated_at: String,
}

impl Account {
    /// Whether this account can hold postings, given whether it has
    /// children: postable exactly when it is a leaf. The inverse of
    /// heading-ness; see the type docs for why this is derived.
    #[must_use]
    pub fn is_postable(&self, has_children: bool) -> bool {
        !has_children
    }

    /// Whether this account carries `tag`.
    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|candidate| candidate == tag)
    }

    /// The display name for `locale`, looked up in the bundled i18n
    /// tables by [`Self::name_key`]; `None` when the key has no entry.
    #[must_use]
    pub fn display_name(&self, locale: Locale) -> Option<&'static str> {
        i18n::translate(&self.name_key, locale)
    }

    /// Validates an account number: 2..=10 ASCII digits and nothing else.
    ///
    /// Leading zeros are allowed — real charts use them (SKR03's bank
    /// accounts live at `0120`…); sortability comes from the chart-wide
    /// fixed width, not from the digits themselves.
    ///
    /// # Errors
    /// [`AccountsError::InvalidNumber`] with the reason when the number
    /// has the wrong length or contains a non-digit.
    pub fn validate_number(number: &str) -> Result<(), AccountsError> {
        let len = number.len();
        if !(2..=10).contains(&len) {
            return Err(AccountsError::InvalidNumber {
                number: number.to_owned(),
                reason: "account numbers are 2 to 10 ASCII digits",
            });
        }
        if !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AccountsError::InvalidNumber {
                number: number.to_owned(),
                reason: "account numbers are ASCII digits only",
            });
        }
        Ok(())
    }

    /// Validates a tag list: each tag matches `^[a-z0-9]+(-[a-z0-9]+)*$`
    /// (non-empty lowercase kebab-case), there are no duplicates, and the
    /// list holds at most [`MAX_TAGS`] tags.
    ///
    /// # Errors
    /// [`AccountsError::InvalidTag`] for a malformed or duplicated tag.
    pub fn validate_tags(tags: &[String]) -> Result<(), AccountsError> {
        if tags.len() > MAX_TAGS {
            return Err(AccountsError::InvalidTag {
                tag: format!("more than {MAX_TAGS} tags on one account"),
            });
        }
        for tag in tags {
            if !is_kebab_tag(tag) {
                return Err(AccountsError::InvalidTag { tag: tag.clone() });
            }
        }
        for (index, tag) in tags.iter().enumerate() {
            if tags[..index].contains(tag) {
                return Err(AccountsError::InvalidTag { tag: tag.clone() });
            }
        }
        Ok(())
    }

    /// Whether a name key is well-formed: it must start with `"account."`
    /// so every translatable account name lives in one namespace (template
    /// names use the `"template."` prefix instead). A predicate rather
    /// than a `Result` because there is exactly one way to fail.
    #[must_use]
    pub fn name_key_is_valid(name_key: &str) -> bool {
        name_key.starts_with("account.")
    }
}

/// An [`Account`] paired with its derived tree state: whether it has
/// children. Everything postable/heading flows from that one bit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AccountEntry {
    /// The stored account row.
    pub account: Account,
    /// Whether any account in the company names this one as parent.
    pub has_children: bool,
}

impl AccountEntry {
    /// Whether postings may target this account: exactly when it is a
    /// leaf. Headings structure the chart; leaves hold balances.
    #[must_use]
    pub fn is_postable(&self) -> bool {
        !self.has_children
    }

    /// Whether this account is a heading: it has children, so it exists
    /// to group, not to post against.
    #[must_use]
    pub fn is_heading(&self) -> bool {
        self.has_children
    }

    /// Whether this account is a leaf (has no children). Same predicate as
    /// [`Self::is_postable`], named for the tree reading.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        !self.has_children
    }
}

/// The input for creating one account: everything the caller chooses,
/// nothing the system owns (no `company_id`, `archived` or timestamps).
/// `normal_balance` defaults from the account type when `None`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AccountDraft {
    /// The account number (validated by [`Account::validate_number`]).
    pub number: String,
    /// The translatable name key (must start with `"account."`).
    pub name_key: String,
    /// The account class.
    pub account_type: AccountType,
    /// The balance side; `None` means [`AccountType::default_normal_balance`].
    pub normal_balance: Option<NormalBalance>,
    /// The account's own currency; `None` = company base currency. Already
    /// a `ledgers-money` `Currency`, so it has passed `from_code` wherever
    /// the code string was parsed.
    pub currency: Option<Currency>,
    /// The parent account number; `None` = root.
    pub parent: Option<String>,
    /// Kebab-case tags (validated by [`Account::validate_tags`]).
    pub tags: Vec<String>,
}

impl AccountDraft {
    /// Validates the draft and freezes it into an [`Account`] owned by
    /// `company_id`, with `now` (ISO 8601 UTC) as both timestamps.
    ///
    /// # Errors
    /// [`AccountsError::InvalidNumber`] for a bad number,
    /// [`AccountsError::InvalidTag`] for a bad tag list, and a name-key
    /// error when the key does not start with `"account."`.
    pub fn into_account(self, company_id: &str, now: &str) -> Result<Account, AccountsError> {
        Account::validate_number(&self.number)?;
        Account::validate_tags(&self.tags)?;
        if !Account::name_key_is_valid(&self.name_key) {
            return Err(AccountsError::InvalidNameKey { key: self.name_key });
        }
        Ok(Account {
            company_id: company_id.to_owned(),
            number: self.number,
            name_key: self.name_key,
            normal_balance: self
                .normal_balance
                .unwrap_or(self.account_type.default_normal_balance()),
            account_type: self.account_type,
            currency: self.currency,
            parent: self.parent,
            tags: self.tags,
            archived: false,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        })
    }
}

/// Whether `tag` matches `^[a-z0-9]+(-[a-z0-9]+)*$`: non-empty lowercase
/// kebab-case — one or more `[a-z0-9]` segments joined by single hyphens.
/// Hand-rolled (no regex dependency): the grammar is four lines.
fn is_kebab_tag(tag: &str) -> bool {
    if tag.is_empty() {
        return false;
    }
    let mut expect_segment = true;
    for byte in tag.bytes() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => expect_segment = false,
            b'-' if !expect_segment => expect_segment = true,
            _ => return false,
        }
    }
    !expect_segment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_normal_balance_follows_the_class() {
        assert_eq!(
            AccountType::Asset.default_normal_balance(),
            NormalBalance::Debit
        );
        assert_eq!(
            AccountType::Expense.default_normal_balance(),
            NormalBalance::Debit
        );
        assert_eq!(
            AccountType::Liability.default_normal_balance(),
            NormalBalance::Credit
        );
        assert_eq!(
            AccountType::Equity.default_normal_balance(),
            NormalBalance::Credit
        );
        assert_eq!(
            AccountType::Income.default_normal_balance(),
            NormalBalance::Credit
        );
    }

    #[test]
    fn account_type_round_trips_through_strings() {
        for text in ["asset", "liability", "equity", "income", "expense"] {
            let parsed: AccountType = text.parse().expect("known class parses");
            assert_eq!(parsed.to_string(), text);
        }
        for text in ["debit", "credit"] {
            let parsed: NormalBalance = text.parse().expect("known balance parses");
            assert_eq!(parsed.to_string(), text);
        }
    }

    #[test]
    fn unknown_class_and_balance_name_the_offending_value() {
        let err = "bank".parse::<AccountType>().unwrap_err();
        assert_eq!(
            err,
            AccountsError::UnknownAccountType {
                value: "bank".to_owned()
            }
        );
        let err = "left".parse::<NormalBalance>().unwrap_err();
        assert_eq!(
            err,
            AccountsError::UnknownNormalBalance {
                value: "left".to_owned()
            }
        );
    }

    #[test]
    fn number_validation_bounds_and_digits() {
        assert!(Account::validate_number("10").is_ok());
        assert!(Account::validate_number("0123456789").is_ok()); // leading zero, 10 wide
        assert!(Account::validate_number("1").is_err()); // too short
        assert!(Account::validate_number("12345678901").is_err()); // too long
        assert!(Account::validate_number("12a4").is_err()); // not a digit
        assert!(Account::validate_number("12 4").is_err());
        assert!(Account::validate_number("").is_err());
    }

    #[test]
    fn tag_validation_accepts_kebab_and_rejects_the_rest() {
        let tags = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        assert!(Account::validate_tags(&tags(&["bank", "vat-input", "fx-gain", "a1"])).is_ok());
        for bad in [
            "",
            "Bank",
            "bank_",
            "-bank",
            "bank-",
            "bank--cash",
            "bank cash",
            "bank.cash",
        ] {
            assert!(
                Account::validate_tags(&tags(&[bad])).is_err(),
                "{bad:?} must be invalid"
            );
        }
        assert!(
            Account::validate_tags(&tags(&["bank", "bank"])).is_err(),
            "duplicates are invalid"
        );
        let many = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect::<Vec<_>>();
        assert!(
            Account::validate_tags(&many).is_err(),
            "more than MAX_TAGS is invalid"
        );
        let sixteen = (0..MAX_TAGS).map(|i| format!("t{i}")).collect::<Vec<_>>();
        assert!(
            Account::validate_tags(&sixteen).is_ok(),
            "exactly MAX_TAGS is fine"
        );
    }

    #[test]
    fn postable_is_derived_from_children_not_stored() {
        let mut draft = AccountDraft {
            number: "01000".to_owned(),
            name_key: "account.generic_sme.assets".to_owned(),
            account_type: AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: Vec::new(),
        };
        let account = draft
            .clone()
            .into_account("co", "2026-01-01T00:00:00Z")
            .expect("valid draft");
        assert_eq!(account.normal_balance, NormalBalance::Debit); // defaulted from type
        assert!(account.is_postable(false));
        assert!(!account.is_postable(true)); // the same row, once a child exists

        draft.normal_balance = Some(NormalBalance::Credit); // contra: asset, credit
        let contra = draft
            .into_account("co", "2026-01-01T00:00:00Z")
            .expect("valid draft");
        assert_eq!(contra.account_type, AccountType::Asset);
        assert_eq!(contra.normal_balance, NormalBalance::Credit);

        let entry = AccountEntry {
            account,
            has_children: true,
        };
        assert!(entry.is_heading());
        assert!(!entry.is_postable());
        assert!(!entry.is_leaf());
    }

    #[test]
    fn draft_validation_rejects_bad_numbers_tags_and_name_keys() {
        let base = |number: &str, key: &str| AccountDraft {
            number: number.to_owned(),
            name_key: key.to_owned(),
            account_type: AccountType::Expense,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: Vec::new(),
        };
        assert!(
            base("5100", "account.generic_sme.rent")
                .into_account("co", "now")
                .is_ok()
        );
        assert!(matches!(
            base("X", "account.generic_sme.rent").into_account("co", "now"),
            Err(AccountsError::InvalidNumber { .. })
        ));
        assert!(matches!(
            base("5100", "template.generic_sme.name").into_account("co", "now"),
            Err(AccountsError::InvalidNameKey { .. })
        ));
    }

    #[test]
    fn has_tag_and_display_name() {
        let account = AccountDraft {
            number: "01000".to_owned(),
            // A well-shaped key that no bundled chart uses: display must
            // be None rather than a guess, whatever the data files hold.
            name_key: "account.test.not-in-any-table".to_owned(),
            account_type: AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec!["bank".to_owned()],
        }
        .into_account("co", "now")
        .expect("valid draft");
        assert!(account.has_tag("bank"));
        assert!(!account.has_tag("cash"));
        assert_eq!(account.display_name(Locale::En), None);
        assert_eq!(account.display_name(Locale::De), None);
    }
}
