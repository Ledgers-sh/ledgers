//! Integration tests for credit note issuance.
mod common;

use cratefield_core::Statement;
use cratefield_testing::TestHarness;
use ledgers_bills::BillsModule;
use ledgers_commands::Command;
use ledgers_db::DbModule;
use ledgers_invoice::InvoiceModule;
use ledgers_invoice::commands::{CreditInvoice, IssueInvoice};
use ledgers_invoice::model::{CreditNoteInput, CreditSpec, IssueInvoiceInput};
use ledgers_ledger::LedgerModule;
use ledgers_storage::StorageModule;
use ledgers_subledger::SubledgerModule;
use sea_query::Value;

use common::{COMPANY_ID, run_command, seed_company_with_periods, sum_journal_line_base};

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![
        Box::new(DbModule),
        Box::new(StorageModule),
        Box::new(LedgerModule),
        Box::new(SubledgerModule),
        Box::new(BillsModule),
        Box::new(InvoiceModule),
    ]
}

fn simple_invoice_input(issue_date: &str) -> IssueInvoiceInput {
    IssueInvoiceInput {
        customer_id: "cust_acme".to_string(),
        series: "RE".to_string(),
        issue_date: issue_date.to_string(),
        supply_date: None,
        payment_terms_days: Some(14),
        currency: None,
        rate: None,
        lines: vec![
            ledgers_invoice::model::InvoiceLineInput {
                description: "Consulting 19%".to_string(),
                quantity: "1".to_string(),
                unit: None,
                unit_price_minor: None,
                net_minor: Some(100_000),
                tax_code: "19-U".to_string(),
                revenue_account: Some("4000".to_string()),
            },
            ledgers_invoice::model::InvoiceLineInput {
                description: "Travel 7%".to_string(),
                quantity: "1".to_string(),
                unit: None,
                unit_price_minor: None,
                net_minor: Some(20_000),
                tax_code: "7-U".to_string(),
                revenue_account: Some("4000".to_string()),
            },
        ],
    }
}

#[test]
fn partial_line_credit_posts_in_its_own_period() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company_with_periods(db).await;

            // Issue invoice dated 2026-01-15
            let inv_input = simple_invoice_input("2026-01-15");
            let inv_output = run_command(db, &IssueInvoice, &inv_input, "test-inv-01")
                .await
                .expect("invoice should post");

            // Load invoice lines to get the IDs
            let lines = db
                .query(&Statement::with_values(
                    "SELECT id FROM invoice_line WHERE invoice_id = ? ORDER BY sequence"
                        .to_string(),
                    vec![Value::from(inv_output.invoice_id.as_str())],
                ))
                .await
                .unwrap();

            let line_1_id = lines.rows[0].get::<String>("id").unwrap();

            // Credit 300.00 of line 1 (the 19% line, 1000.00 net) on 2026-03-20
            let cn_input = CreditNoteInput {
                invoice_id: inv_output.invoice_id.clone(),
                series: "GS".to_string(),
                issue_date: "2026-03-20".to_string(),
                reason: Some("Partial return".to_string()),
                credit_spec: CreditSpec::Lines {
                    lines: vec![ledgers_invoice::model::CreditLineInput {
                        invoice_line_id: line_1_id.clone(),
                        net_minor: 30_000,
                    }],
                },
            };

            let cn_output = run_command(db, &CreditInvoice, &cn_input, "test-cn-01")
                .await
                .expect("credit note should post");

            // Verify entry balances: AR credited (gross), revenue debited (net), VAT debited
            let (debit, credit) = sum_journal_line_base(db, &cn_output.entry_id).await;
            assert_eq!(debit, credit, "entry must balance");

            // AR credit should be gross: 300 net + (300 * 0.19) tax = 357
            assert_eq!(credit, 35_700);
            assert_eq!(debit, 35_700);

            // Verify open item
            let oi = db
                .query(&Statement::with_values(
                    "SELECT tx_original_minor FROM open_item WHERE source_id = ?".to_string(),
                    vec![Value::from(cn_output.credit_note_number.as_str())],
                ))
                .await
                .unwrap();

            assert_eq!(oi.rows.len(), 1);
            let oi_amount = oi.rows[0].get::<i64>("tx_original_minor").unwrap().abs();
            assert_eq!(oi_amount, 35_700);

            // Verify credit note numbering
            assert_eq!(cn_output.credit_note_number, "GS-2026-0001");

            // Verify invoice rows unchanged
            let inv_check = db
                .query(&Statement::with_values(
                    "SELECT net_minor FROM invoice_line WHERE id = ?".to_string(),
                    vec![Value::from(line_1_id.as_str())],
                ))
                .await
                .unwrap();

            assert_eq!(inv_check.rows[0].get::<i64>("net_minor").unwrap(), 10_0000);
        });
    }
}

#[test]
fn over_crediting_a_line_is_refused() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company_with_periods(db).await;

            // Issue invoice
            let inv_input = simple_invoice_input("2026-01-15");
            let inv_output = run_command(db, &IssueInvoice, &inv_input, "test-inv-02")
                .await
                .expect("invoice should post");

            let lines = db
                .query(&Statement::with_values(
                    "SELECT id FROM invoice_line WHERE invoice_id = ? ORDER BY sequence"
                        .to_string(),
                    vec![Value::from(inv_output.invoice_id.as_str())],
                ))
                .await
                .unwrap();

            let line_1_id = lines.rows[0].get::<String>("id").unwrap();

            // First credit: 800.00
            let cn1_input = CreditNoteInput {
                invoice_id: inv_output.invoice_id.clone(),
                series: "GS".to_string(),
                issue_date: "2026-03-20".to_string(),
                reason: None,
                credit_spec: CreditSpec::Lines {
                    lines: vec![ledgers_invoice::model::CreditLineInput {
                        invoice_line_id: line_1_id.clone(),
                        net_minor: 80_000,
                    }],
                },
            };

            let cn1_output = run_command(db, &CreditInvoice, &cn1_input, "test-cn-02a")
                .await
                .expect("first credit should succeed");

            assert_eq!(cn1_output.credit_note_number, "GS-2026-0001");

            // Second credit: 300.00 (total would be 1100, exceeds 1000)
            let cn2_input = CreditNoteInput {
                invoice_id: inv_output.invoice_id.clone(),
                series: "GS".to_string(),
                issue_date: "2026-03-20".to_string(),
                reason: None,
                credit_spec: CreditSpec::Lines {
                    lines: vec![ledgers_invoice::model::CreditLineInput {
                        invoice_line_id: line_1_id.clone(),
                        net_minor: 30_000,
                    }],
                },
            };

            let cn2_result = run_command(db, &CreditInvoice, &cn2_input, "test-cn-02b").await;
            assert!(
                cn2_result.is_err(),
                "second credit should be refused (over-credit)"
            );

            // Verify counter didn't advance for the failed second credit
            let counter = db
                .query(&Statement::with_values(
                    "SELECT next_number FROM credit_note_number_counter \
                     WHERE company_id = ? AND series = ? AND fiscal_year = ?"
                        .to_string(),
                    vec![
                        Value::from(COMPANY_ID),
                        Value::from("GS"),
                        Value::from(2026i64),
                    ],
                ))
                .await
                .unwrap();

            assert_eq!(
                counter.rows[0].get::<i64>("next_number").unwrap(),
                2,
                "next credit note should be 0002"
            );
        });
    }
}

#[test]
fn amount_credit_is_spread_pro_rata() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company_with_periods(db).await;

            // Issue invoice: 1000.00 at 19% + 200.00 at 7% = 1200.00 net
            let inv_input = simple_invoice_input("2026-01-15");
            let inv_output = run_command(db, &IssueInvoice, &inv_input, "test-inv-03")
                .await
                .expect("invoice should post");

            // Credit 600.00 net spread pro-rata across both lines
            // Line 1: 1000 / 1200 * 600 = 500
            // Line 2: 200 / 1200 * 600 = 100
            let cn_input = CreditNoteInput {
                invoice_id: inv_output.invoice_id.clone(),
                series: "GS".to_string(),
                issue_date: "2026-03-20".to_string(),
                reason: None,
                credit_spec: CreditSpec::Amount {
                    amount_minor: 60_000,
                },
            };

            let cn_output = run_command(db, &CreditInvoice, &cn_input, "test-cn-03")
                .await
                .expect("pro-rata credit should succeed");

            // Verify total credited equals what was specified
            assert_eq!(cn_output.total_credited_minor, 60_000);

            // Verify the credit note lines have correct net amounts
            let cn_lines = db
                .query(&Statement::with_values(
                    "SELECT net_credited_minor FROM credit_note_line WHERE credit_note_id = ? \
                     ORDER BY id"
                        .to_string(),
                    vec![Value::from(cn_output.credit_note_id.as_str())],
                ))
                .await
                .unwrap();

            assert_eq!(cn_lines.rows.len(), 2);
            let line1_credited = cn_lines.rows[0].get::<i64>("net_credited_minor").unwrap();
            let line2_credited = cn_lines.rows[1].get::<i64>("net_credited_minor").unwrap();

            assert_eq!(line1_credited, 50_000);
            assert_eq!(line2_credited, 10_000);
        });
    }
}

#[test]
fn credit_note_rows_are_immutable() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company_with_periods(db).await;

            // Issue invoice
            let inv_input = simple_invoice_input("2026-01-15");
            let inv_output = run_command(db, &IssueInvoice, &inv_input, "test-inv-04")
                .await
                .expect("invoice should post");

            let lines = db
                .query(&Statement::with_values(
                    "SELECT id FROM invoice_line WHERE invoice_id = ? ORDER BY sequence"
                        .to_string(),
                    vec![Value::from(inv_output.invoice_id.as_str())],
                ))
                .await
                .unwrap();

            let line_1_id = lines.rows[0].get::<String>("id").unwrap();

            // Create credit note
            let cn_input = CreditNoteInput {
                invoice_id: inv_output.invoice_id.clone(),
                series: "GS".to_string(),
                issue_date: "2026-03-20".to_string(),
                reason: None,
                credit_spec: CreditSpec::Lines {
                    lines: vec![ledgers_invoice::model::CreditLineInput {
                        invoice_line_id: line_1_id,
                        net_minor: 30_000,
                    }],
                },
            };

            let cn_output = run_command(db, &CreditInvoice, &cn_input, "test-cn-04")
                .await
                .expect("credit note should post");

            // Try to UPDATE credit_note
            let update_result = db
                .execute(&Statement::with_values(
                    "UPDATE credit_note SET reason = ? WHERE id = ?".to_string(),
                    vec![
                        Value::from("modified"),
                        Value::from(cn_output.credit_note_id.as_str()),
                    ],
                ))
                .await;

            assert!(update_result.is_err(), "update on credit_note should fail");

            // Try to DELETE credit_note
            let delete_result = db
                .execute(&Statement::with_values(
                    "DELETE FROM credit_note WHERE id = ?".to_string(),
                    vec![Value::from(cn_output.credit_note_id.as_str())],
                ))
                .await;

            assert!(delete_result.is_err(), "delete on credit_note should fail");

            // Try to UPDATE credit_note_line
            let cn_lines = db
                .query(&Statement::with_values(
                    "SELECT id FROM credit_note_line WHERE credit_note_id = ?".to_string(),
                    vec![Value::from(cn_output.credit_note_id.as_str())],
                ))
                .await
                .unwrap();

            let line_id = cn_lines.rows[0].get::<String>("id").unwrap();

            let update_line_result = db
                .execute(&Statement::with_values(
                    "UPDATE credit_note_line SET net_credited_minor = ? WHERE id = ?".to_string(),
                    vec![Value::from(40_000i64), Value::from(line_id.as_str())],
                ))
                .await;

            assert!(
                update_line_result.is_err(),
                "update on credit_note_line should fail"
            );

            // Try to DELETE credit_note_line
            let delete_line_result = db
                .execute(&Statement::with_values(
                    "DELETE FROM credit_note_line WHERE id = ?".to_string(),
                    vec![Value::from(line_id.as_str())],
                ))
                .await;

            assert!(
                delete_line_result.is_err(),
                "delete on credit_note_line should fail"
            );
        });
    }
}

#[test]
fn credit_preview_lines_match_posted_lines() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company_with_periods(db).await;

            // Issue invoice
            let inv_input = simple_invoice_input("2026-01-15");
            let inv_output = run_command(db, &IssueInvoice, &inv_input, "test-inv-01")
                .await
                .expect("invoice should post");

            // Load invoice lines to get the IDs
            let lines = db
                .query(&Statement::with_values(
                    "SELECT id FROM invoice_line WHERE invoice_id = ? ORDER BY sequence"
                        .to_string(),
                    vec![Value::from(inv_output.invoice_id.as_str())],
                ))
                .await
                .unwrap();

            let line_1_id = lines.rows[0].get::<String>("id").unwrap();

            // Create credit note input
            let cn_input = CreditNoteInput {
                invoice_id: inv_output.invoice_id.clone(),
                series: "GS".to_string(),
                issue_date: "2026-03-20".to_string(),
                reason: Some("Partial return".to_string()),
                credit_spec: CreditSpec::Lines {
                    lines: vec![ledgers_invoice::model::CreditLineInput {
                        invoice_line_id: line_1_id.clone(),
                        net_minor: 30_000,
                    }],
                },
            };

            // Get preview of what lines will be posted
            let ctx = ledgers_commands::CommandContext {
                company_id: common::COMPANY_ID.to_string(),
                request_id: "test-credit-preview".to_string(),
                event_id: "test-credit-preview-event".to_string(),
                now: common::NOW.to_string(),
                is_person: true,
            };
            let preview = CreditInvoice
                .preview(db, &ctx, &cn_input)
                .await
                .expect("preview should succeed");

            // Extract account and side from preview lines, sorted for comparison
            let mut preview_lines: Vec<_> = preview
                .lines
                .iter()
                .map(|line| (line.account.clone(), line.side.clone(), line.base.clone()))
                .collect();
            preview_lines.sort_by_key(|a| (a.0.clone(), a.1.clone()));

            // Post the credit note and get the posted lines
            let cn_output = run_command(db, &CreditInvoice, &cn_input, "test-credit-preview")
                .await
                .expect("credit note should post");

            let posted_lines = common::get_journal_lines(db, &cn_output.entry_id).await;
            #[allow(clippy::cast_precision_loss)]
            let mut posted_formatted: Vec<_> = posted_lines
                .iter()
                .map(|(account, side, amount)| {
                    (
                        account.clone(),
                        side.clone(),
                        // Convert minor units to canonical format (EUR: divide by 100 and format with 2 decimals)
                        format!("{:.2}", *amount as f64 / 100.0),
                    )
                })
                .collect();
            posted_formatted.sort_by_key(|a| (a.0.clone(), a.1.clone()));

            // Both should have the same number of lines
            assert_eq!(
                preview_lines.len(),
                posted_formatted.len(),
                "preview and posted should have same number of lines"
            );

            // Each line should match
            for (i, (prev_line, posted_line)) in preview_lines
                .iter()
                .zip(posted_formatted.iter())
                .enumerate()
            {
                assert_eq!(
                    prev_line.0, posted_line.0,
                    "line {} account mismatch: preview {} vs posted {}",
                    i, prev_line.0, posted_line.0
                );
                assert_eq!(
                    prev_line.1, posted_line.1,
                    "line {} side mismatch: preview {} vs posted {}",
                    i, prev_line.1, posted_line.1
                );
                assert_eq!(
                    prev_line.2, posted_line.2,
                    "line {} amount mismatch: preview {} vs posted {}",
                    i, prev_line.2, posted_line.2
                );
            }
        });
    }
}
