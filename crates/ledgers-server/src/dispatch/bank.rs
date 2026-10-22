//! The bank commands' server side (issues #26 and #48). The statement import
//! reads the uploaded document's bytes and parses them here, so the command
//! only ever receives statements the server read from a stored file. Who
//! acts is filled from the credential: a client cannot name the person, nor
//! make its own rule apply.

use super::{Deps, ServerDispatch};
use crate::routes::Presented;
use cratefield_core::Problem;
use ledgers_bank::commands::{
    AccountsArchive, AccountsCreate, AccountsGet, AccountsList, AccountsReconcile, AccountsUpdate,
    RulesCreate, RulesEnable, RulesList, StatementsImport, TransactionsIgnore, TransactionsLink,
    TransactionsList, TransactionsPost, rule_request_to_input,
};
use ledgers_bank::model::{RulesCreateRequest, StatementsImportInput, StatementsImportRequest};
use ledgers_bank::parse;
use ledgers_bank::statement::Format;
use ledgers_commands::CommandError;
use serde_json::Value;

impl ServerDispatch {
    /// The account, rules and reconciliation writes that a person makes:
    /// the server sets the acting person on the input.
    pub(super) async fn bank_person_write(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        // A person-only command: an agent key is refused here, before anything else.
        let person = self
            .person_id(deps, presented.clone(), request_id, Some(command))
            .await?;
        if let Value::Object(map) = &mut input {
            map.insert("actor".to_owned(), Value::String(person));
        }
        match command {
            "bank.accounts.create" => {
                self.execute(deps, &AccountsCreate, presented, request_id, input)
                    .await
            }
            "bank.accounts.update" => {
                self.execute(deps, &AccountsUpdate, presented, request_id, input)
                    .await
            }
            "bank.accounts.archive" => {
                self.execute(deps, &AccountsArchive, presented, request_id, input)
                    .await
            }
            "bank.rules.enable" => {
                self.execute(deps, &RulesEnable, presented, request_id, input)
                    .await
            }
            other => Err(
                Problem::validation_failed(format!("unknown command {other:?}"))
                    .instance(request_id),
            ),
        }
    }

    /// Dispatches the bank commands that need no server-side preparation.
    pub(super) async fn bank_command(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        match command {
            "bank.accounts.list" => {
                self.execute(deps, &AccountsList, presented, request_id, input)
                    .await
            }
            "bank.accounts.get" => {
                self.execute(deps, &AccountsGet, presented, request_id, input)
                    .await
            }
            "bank.accounts.reconcile" => {
                self.execute(deps, &AccountsReconcile, presented, request_id, input)
                    .await
            }
            "bank.transactions.list" => {
                self.execute(deps, &TransactionsList, presented, request_id, input)
                    .await
            }
            "bank.transactions.post" => {
                self.execute(deps, &TransactionsPost, presented, request_id, input)
                    .await
            }
            "bank.transactions.link" => {
                self.execute(deps, &TransactionsLink, presented, request_id, input)
                    .await
            }
            "bank.transactions.ignore" => {
                self.execute(deps, &TransactionsIgnore, presented, request_id, input)
                    .await
            }
            "bank.rules.list" => {
                self.execute(deps, &RulesList, presented, request_id, input)
                    .await
            }
            "bank.rules.create" => {
                self.bank_rules_create(deps, presented, request_id, input)
                    .await
            }
            "bank.statements.import" => {
                self.bank_statements_import(deps, presented, request_id, input)
                    .await
            }
            "bank.accounts.create"
            | "bank.accounts.update"
            | "bank.accounts.archive"
            | "bank.rules.enable" => {
                self.bank_person_write(deps, presented, request_id, command, input)
                    .await
            }
            other => Err(
                Problem::validation_failed(format!("unknown command {other:?}"))
                    .instance(request_id),
            ),
        }
    }

    /// `bank.statements.import`: reads the stored document, parses it with the
    /// account's mapping when it is a CSV, and imports the statements.
    async fn bank_statements_import(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let request: StatementsImportRequest = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let caller = self
            .caller_label(deps, presented.as_ref(), request_id)
            .await?;
        let db = deps.db.as_ref();
        let company = self.company_id();
        let account = ledgers_bank::store::account(db, company, &request.account_id)
            .await
            .map_err(|err| fail(CommandError::Database(err)))?
            .ok_or_else(|| fail(CommandError::Domain(Problem::not_found())))?;
        let document = ledgers_documents::store::get(db, company, &request.document_id)
            .await
            .map_err(|err| fail(CommandError::Database(err)))?
            .ok_or_else(|| {
                fail(CommandError::InvalidInput(format!(
                    "document {} is not stored",
                    request.document_id
                )))
            })?;
        // The statement's text is kept in the document row at upload, so the
        // import reads the same database the document lives in.
        let text = ledgers_documents::store::text_layer(db, company, &document.id)
            .await
            .map_err(|err| fail(CommandError::Database(err)))?
            .ok_or_else(|| {
                fail(CommandError::InvalidInput(
                    "the document has no statement text: upload a camt.053, MT940 or CSV file"
                        .to_owned(),
                ))
            })?;
        let bytes = text.as_bytes();
        let format = match request.format {
            Some(format) => format,
            None => parse::detect(bytes).ok_or_else(|| {
                fail(CommandError::InvalidInput(
                    "the format is not recognised: name it (camt053, mt940 or csv)".to_owned(),
                ))
            })?,
        };
        let mapping = (format == Format::Csv)
            .then(|| account.csv_mapping.clone())
            .flatten();
        if format == Format::Csv && mapping.is_none() {
            return Err(fail(CommandError::InvalidInput(
                "this account has no CSV column mapping: set one first".to_owned(),
            )));
        }
        let statements = parse::parse(format, bytes, mapping.as_ref(), &account.currency)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let command_input = StatementsImportInput {
            account_id: request.account_id,
            document_id: request.document_id,
            statements,
            actor: caller,
        };
        let value = serde_json::to_value(command_input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        self.execute(deps, &StatementsImport, presented, request_id, value)
            .await
    }

    /// `bank.rules.create`: the server sets the id and who proposed the rule.
    /// A person's rule applies at once; an agent's waits for a person.
    async fn bank_rules_create(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let request: RulesCreateRequest = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let caller = self
            .caller_label(deps, presented.as_ref(), request_id)
            .await?;
        let enabled = caller.starts_with("person:");
        let command_input = rule_request_to_input(request, deps.id_gen.ulid(), caller, enabled);
        let value = serde_json::to_value(command_input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        self.execute(deps, &RulesCreate, presented, request_id, value)
            .await
    }
}
