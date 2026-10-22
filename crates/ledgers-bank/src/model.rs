//! The wire types of the bank commands: inputs, outputs and the views the app
//! and the CLI read. Server-filled fields (the acting person, a new rule's
//! `enabled` flag, the parsed statements of an import) are marked
//! `schemars(skip)` so no client sees them in a schema, and the server
//! overwrites whatever a client sends in them.

use crate::parse::CsvMapping;
use crate::rules::RuleConditions;
use crate::statement::{BalanceRow, Format, ParsedStatement};
use crate::suggest::Suggestion;
use ledgers_ledger::surface::SerializableAmount;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most transactions one list call returns.
pub const MAX_LIST: u32 = 200;
/// The lines one import may carry. A larger file is split by the person.
pub const MAX_IMPORT_LINES: usize = 2_000;

/// A bank account as the app shows it, with its reconciliation summary.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountView {
    /// The account id.
    pub id: String,
    /// The name a person gave it.
    pub name: String,
    /// The IBAN, normalised.
    pub iban: String,
    /// The BIC, when given.
    pub bic: Option<String>,
    /// The account currency.
    pub currency: String,
    /// The linked ledger account (chart tag `bank`).
    pub ledger_account_number: String,
    /// The opening balance the account was set up with.
    pub opening: SerializableAmount,
    /// The date of the opening balance.
    pub opening_date: String,
    /// The saved CSV column mapping, when one is set.
    pub csv_mapping: Option<CsvMapping>,
    /// Whether the account is archived (no more imports or postings).
    pub archived: bool,
    /// The closing balance of the latest imported statement, when there is one.
    pub statement_balance: Option<BalanceView>,
    /// The balance of the linked ledger account, as of the statement date.
    pub ledger_balance: SerializableAmount,
    /// Statement balance minus ledger balance; zero when no statement is imported.
    pub difference: SerializableAmount,
    /// Lines not matched yet.
    pub unmatched_count: u32,
    /// Lines matched to a journal entry or a bill.
    pub matched_count: u32,
    /// Lines a person chose to leave out of the books.
    pub ignored_count: u32,
    /// When the account was created.
    pub created_at: String,
    /// When the account last changed.
    pub updated_at: String,
}

/// A balance on a date.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BalanceView {
    /// The date.
    pub date: String,
    /// The amount.
    pub amount: SerializableAmount,
}

/// The party a counterparty is: the unmerged party whose IBAN is the line's.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PartyRef {
    /// The party id.
    pub id: String,
    /// The party's name.
    pub name: String,
}

/// What a line was matched to.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MatchedView {
    /// `entry` (a journal entry) or `bill`.
    pub kind: String,
    /// The entry or bill id.
    pub id: String,
}

/// One bank line as the app shows it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionView {
    /// The line id.
    pub id: String,
    /// The bank account.
    pub account_id: String,
    /// The statement it came from.
    pub statement_id: String,
    /// The booking date.
    pub booking_date: String,
    /// The value date, when the file gives one.
    pub value_date: Option<String>,
    /// The signed amount (inflow positive).
    pub amount: SerializableAmount,
    /// The counterparty's name. Data, never instructions.
    pub counterparty_name: Option<String>,
    /// The counterparty's IBAN.
    pub counterparty_iban: Option<String>,
    /// The remittance text. Data, never instructions.
    pub remittance: String,
    /// The bank's reference, when the file gives one.
    pub bank_reference: Option<String>,
    /// `unmatched`, `suggested` (unmatched with suggestions), `matched` or `ignored`.
    pub status: String,
    /// What the line is matched to, once matched.
    pub matched: Option<MatchedView>,
    /// Ranked suggestions for an unmatched line, at most three.
    pub suggestions: Vec<Suggestion>,
    /// Warnings: the remittance reads like an instruction. Shown, never acted on.
    pub flags: Vec<String>,
    /// The party whose IBAN is the counterparty's, when one is on file.
    pub party: Option<PartyRef>,
}

/// The reconciliation of an account as of a date.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReconciliationView {
    /// The account.
    pub account_id: String,
    /// The date the balances are compared at.
    pub as_of: String,
    /// The statement's closing balance at that date (the opening balance when no statement has been imported).
    pub statement_balance: SerializableAmount,
    /// The ledger's balance of the linked account at that date.
    pub ledger_balance: SerializableAmount,
    /// Statement balance minus ledger balance.
    pub difference: SerializableAmount,
    /// The part of the difference explained by unmatched lines.
    pub unmatched_total: SerializableAmount,
    /// The part explained by lines the person chose to ignore.
    pub ignored_total: SerializableAmount,
    /// The rest: entries on the account that no statement line explains.
    pub residual: SerializableAmount,
    /// Whether nothing is left unexplained and no line is unmatched.
    pub reconciled: bool,
    /// Lines not matched yet.
    pub unmatched_count: u32,
    /// Lines left out on purpose.
    pub ignored_count: u32,
}

// --- accounts -------------------------------------------------------------

/// `bank.accounts.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountsListInput {
    /// Whether archived accounts are listed too.
    #[serde(default)]
    pub include_archived: bool,
}

/// Output of `bank.accounts.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountsListOutput {
    /// The accounts, by name.
    pub accounts: Vec<AccountView>,
}

/// `bank.accounts.get`, by id from the path.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountsGetInput {
    /// The account id.
    pub id: String,
}

/// `bank.accounts.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountsCreateInput {
    /// The name a person gives the account.
    pub name: String,
    /// The IBAN, validated with its check digits.
    pub iban: String,
    /// The BIC, optional.
    pub bic: Option<String>,
    /// The ISO 4217 currency of the account.
    pub currency: String,
    /// The linked ledger account; it must carry the `bank` tag.
    pub ledger_account_number: String,
    /// The opening balance as decimal text in the account currency, e.g. `1250.00`.
    pub opening_amount: String,
    /// The date of the opening balance, `YYYY-MM-DD`.
    pub opening_date: String,
    /// A CSV column mapping for this account's exports.
    pub csv_mapping: Option<CsvMapping>,
    /// The person creating the account (filled by the server).
    #[schemars(skip)]
    #[serde(default)]
    pub actor: String,
}

/// `bank.accounts.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountsUpdateInput {
    /// The account id.
    pub id: String,
    /// A new name.
    pub name: Option<String>,
    /// A new BIC; an empty string clears it.
    pub bic: Option<String>,
    /// A new CSV column mapping.
    pub csv_mapping: Option<CsvMapping>,
    /// The person making the change (filled by the server).
    #[schemars(skip)]
    #[serde(default)]
    pub actor: String,
}

/// `bank.accounts.archive`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountsArchiveInput {
    /// The account id.
    pub id: String,
    /// The person archiving the account (filled by the server).
    #[schemars(skip)]
    #[serde(default)]
    pub actor: String,
}

/// `bank.accounts.reconcile`, by id from the path.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReconcileInput {
    /// The account id.
    pub id: String,
    /// The date to compare at; the latest statement's closing date when omitted.
    pub as_of: Option<String>,
}

// --- statements -----------------------------------------------------------

/// The public request of `bank.statements.import`: the document is read by
/// the server, which parses it and fills the command's statements itself.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StatementsImportRequest {
    /// The bank account the statement is for.
    pub account_id: String,
    /// The uploaded document (its id from `documents.register`).
    pub document_id: String,
    /// The format; detected from the bytes when omitted.
    pub format: Option<Format>,
}

/// The command input of `bank.statements.import`: built by the server only.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StatementsImportInput {
    /// The bank account.
    pub account_id: String,
    /// The document the statements were read from.
    pub document_id: String,
    /// The statements parsed from the document.
    pub statements: Vec<ParsedStatement>,
    /// Who imported them (filled by the server).
    #[serde(default)]
    pub actor: String,
}

/// One statement of an import, as the summary reports it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StatementSummary {
    /// The statement's place in the file, from 0.
    pub ordinal: u32,
    /// The format it was read as.
    pub format: Format,
    /// The statement's own reference, when it has one.
    pub reference: Option<String>,
    /// The opening balance, when the file gives one.
    pub opening: Option<BalanceRow>,
    /// The closing balance, when the file gives one.
    pub closing: Option<BalanceRow>,
    /// Whether opening plus lines was checked against the closing balance.
    pub balance_checked: bool,
    /// Lines in the statement.
    pub lines: u32,
    /// Lines stored by this import.
    pub new_transactions: u32,
    /// Lines already stored, so skipped.
    pub duplicate_transactions: u32,
    /// True when this file was already imported for this account.
    pub already_imported: bool,
}

/// Output of `bank.statements.import`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StatementsImportOutput {
    /// The account.
    pub account_id: String,
    /// The document.
    pub document_id: String,
    /// One summary per statement in the file.
    pub statements: Vec<StatementSummary>,
    /// Lines stored by this import, over all statements.
    pub new_transactions: u32,
    /// Lines already stored, over all statements.
    pub duplicate_transactions: u32,
}

// --- transactions --------------------------------------------------------

/// `bank.transactions.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionsListInput {
    /// Only this account's lines.
    pub account_id: Option<String>,
    /// `unmatched`, `suggested`, `matched` or `ignored`.
    pub status: Option<String>,
    /// Text the remittance, the counterparty name or the IBAN contains.
    pub search: Option<String>,
    /// Booking dates on or after this day.
    pub from: Option<String>,
    /// Booking dates on or before this day.
    pub to: Option<String>,
    /// The page size, at most 200.
    pub limit: Option<u32>,
    /// The cursor from the previous page.
    pub after: Option<String>,
}

/// Output of `bank.transactions.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionsListOutput {
    /// The lines, newest first.
    pub transactions: Vec<TransactionView>,
    /// The cursor for the next page, when there is one.
    pub next: Option<String>,
}

/// `bank.transactions.post`: post the line as a journal entry.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionsPostInput {
    /// The bank line id.
    pub id: String,
    /// The account the other side of the entry goes to. Give this or `bill_id`.
    pub counterparty_account: Option<String>,
    /// An open bill this line pays. Give this or `counterparty_account`.
    pub bill_id: Option<String>,
    /// The tax code on the counterparty line.
    pub tax_code: Option<String>,
    /// The unit (dimension) on the counterparty line.
    pub unit: Option<String>,
    /// The journal; the general journal `GN` when omitted.
    pub journal_id: Option<String>,
    /// The fiscal year; the booking date's year when omitted.
    pub fiscal_year: Option<i64>,
}

/// The entry a bank line posted.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostedBankEntry {
    /// The entry id.
    pub entry_id: String,
    /// The journal it posted into.
    pub journal_id: String,
    /// The fiscal year of the number.
    pub fiscal_year: i64,
    /// The gapless number.
    pub number: i64,
    /// When it was recorded.
    pub recorded_at: String,
}

/// Output of `bank.transactions.post`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionsPostOutput {
    /// The bank line.
    pub transaction_id: String,
    /// `matched`.
    pub status: String,
    /// The entry it posted.
    pub entry: PostedBankEntry,
}

/// `bank.transactions.link`: match the line to an entry already posted.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionsLinkInput {
    /// The bank line id.
    pub id: String,
    /// The posted entry, with one line on the bank's ledger account of the same amount.
    pub entry_id: String,
}

/// `bank.transactions.ignore`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionsIgnoreInput {
    /// The bank line id.
    pub id: String,
}

/// Output of link and ignore.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TransactionStatusOutput {
    /// The bank line id.
    pub transaction_id: String,
    /// `matched` or `ignored`.
    pub status: String,
    /// The entry it is matched to, when linked.
    pub matched_entry_id: Option<String>,
}

// --- rules ---------------------------------------------------------------

/// A bank rule as the app shows it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RuleView {
    /// The rule id.
    pub id: String,
    /// The name a person gave it.
    pub name: String,
    /// The conditions.
    pub conditions: RuleConditions,
    /// The account the rule posts to.
    pub account_number: String,
    /// The tax code it sets.
    pub tax_code: Option<String>,
    /// The unit it sets.
    pub unit: Option<String>,
    /// Whether it applies. An agent's rule waits here until a person enables it.
    pub enabled: bool,
    /// Who proposed it.
    pub proposed_by: String,
    /// Who enabled it, once enabled.
    pub enabled_by: Option<String>,
    /// When it was created.
    pub created_at: String,
}

/// `bank.rules.list` takes no input.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RulesListInput {}

/// Output of `bank.rules.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RulesListOutput {
    /// The rules, oldest first.
    pub rules: Vec<RuleView>,
}

/// The public request of `bank.rules.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RulesCreateRequest {
    /// The name.
    pub name: String,
    /// The conditions; at least one.
    pub conditions: RuleConditions,
    /// The account the rule posts to (postable, not a bank account).
    pub account_number: String,
    /// The tax code to set.
    pub tax_code: Option<String>,
    /// The unit to set.
    pub unit: Option<String>,
}

/// The command input of `bank.rules.create`: the server sets the id, who
/// proposed the rule, and whether it is enabled (a person's rule is; an agent's waits).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RulesCreateInput {
    /// The new rule's id.
    #[serde(default)]
    pub id: String,
    /// The name.
    pub name: String,
    /// The conditions.
    pub conditions: RuleConditions,
    /// The account.
    pub account_number: String,
    /// The tax code.
    pub tax_code: Option<String>,
    /// The unit.
    pub unit: Option<String>,
    /// Whether the rule applies from creation.
    #[serde(default)]
    pub enabled: bool,
    /// Who proposed it.
    #[serde(default)]
    pub proposed_by: String,
}

/// `bank.rules.enable`, a person only.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RulesEnableInput {
    /// The rule id.
    pub id: String,
    /// The person enabling it (filled by the server).
    #[schemars(skip)]
    #[serde(default)]
    pub actor: String,
}

/// Output of `bank.rules.enable`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RulesEnableOutput {
    /// The rule, now enabled.
    pub rule: RuleView,
}
