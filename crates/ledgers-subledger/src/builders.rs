//! Builder functions for subledger items and allocations, exposed for use by bills and other commands.

use cratefield_core::Statement;
use ledgers_db::guard;
use sea_query::Value;

/// Builds an INSERT statement for an `open_item` row (bill, invoice, credit note, or on-account).
/// Used by bills.post-entry to create AP open items when posting a bill.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn build_open_item_insert(
    item_id: &str,
    company_id: &str,
    ledger: &str,
    kind: &str,
    party_id: &str,
    source_id: Option<&str>,
    entry_id: &str,
    control_account: &str,
    currency: &str,
    rate_scaled: Option<&str>,
    document_date: &str,
    posting_date: &str,
    due_date: Option<&str>,
    tx_minor: i64,
    base_minor: i64,
    now: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO open_item (id, company_id, ledger, kind, party_id, source_id, entry_id, \
         control_account, currency, rate_scaled, document_date, posting_date, due_date, \
         tx_original_minor, base_original_minor, tx_remaining_minor, base_remaining_minor, \
         created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(item_id),
            Value::from(company_id),
            Value::from(ledger),
            Value::from(kind),
            Value::from(party_id),
            source_id.map_or_else(|| Value::String(None), Value::from),
            Value::from(entry_id),
            Value::from(control_account),
            Value::from(currency),
            rate_scaled.map_or_else(|| Value::String(None), Value::from),
            Value::from(document_date),
            Value::from(posting_date),
            due_date.map_or_else(|| Value::String(None), Value::from),
            Value::from(tx_minor),
            Value::from(base_minor),
            Value::from(tx_minor),
            Value::from(base_minor),
            Value::from(now),
            Value::from(now),
        ],
    )
}

/// Builds an allocation INSERT statement for a single allocation row.
/// Used by bills.record-payment to record payments and other subledger allocation operations.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn build_allocation_insert(
    allocation_id: &str,
    company_id: &str,
    open_item_id: &str,
    entry_id: &str,
    kind: &str,
    tx_minor: i64,
    base_minor: i64,
    fx_difference_minor: i64,
    date: &str,
    now: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO allocation (id, company_id, open_item_id, entry_id, kind, date, \
         tx_minor, base_minor, fx_difference_minor, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(allocation_id),
            Value::from(company_id),
            Value::from(open_item_id),
            Value::from(entry_id),
            Value::from(kind),
            Value::from(date),
            Value::from(tx_minor),
            Value::from(base_minor),
            Value::from(fx_difference_minor),
            Value::from(now),
        ],
    )
}

/// Builds a guard statement for an `open_item` allocation (verify remaining balance).
/// Returns `(guard_key, guard_statement)`.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn build_allocation_guard(
    guard_key_base: &str,
    entry_id: &str,
    open_item_id: &str,
    company_id: &str,
    ledger: &str,
    party_id: &str,
    tx_remaining: i64,
    base_remaining: i64,
) -> (String, Statement) {
    let guard_key = guard::guard_key(&[guard_key_base, entry_id, open_item_id]);
    let stmt = guard::assert_statement(
        &guard_key,
        "(SELECT 1 FROM open_item WHERE id = ? AND company_id = ? AND ledger = ? \
         AND party_id = ? AND tx_remaining_minor = ? AND base_remaining_minor = ?) IS NOT NULL",
        vec![
            Value::from(open_item_id),
            Value::from(company_id),
            Value::from(ledger),
            Value::from(party_id),
            Value::from(tx_remaining),
            Value::from(base_remaining),
        ],
    );
    (guard_key, stmt)
}

/// Builds an UPDATE statement to decrement an `open_item`'s remaining balance.
#[must_use]
pub fn build_open_item_decrement(
    open_item_id: &str,
    company_id: &str,
    ledger: &str,
    party_id: &str,
    tx_decrement: i64,
    base_decrement: i64,
    now: &str,
) -> Statement {
    Statement::with_values(
        "UPDATE open_item SET tx_remaining_minor = tx_remaining_minor - ?, \
         base_remaining_minor = base_remaining_minor - ?, updated_at = ? \
         WHERE id = ? AND company_id = ? AND ledger = ? AND party_id = ?"
            .to_owned(),
        vec![
            Value::from(tx_decrement),
            Value::from(base_decrement),
            Value::from(now),
            Value::from(open_item_id),
            Value::from(company_id),
            Value::from(ledger),
            Value::from(party_id),
        ],
    )
}
