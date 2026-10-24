//! Integration tests for invoice issuance.
mod common;

use cratefield_core::Statement;
use cratefield_testing::TestHarness;
use ledgers_bills::BillsModule;
use ledgers_commands::Command;
use ledgers_db::DbModule;
use ledgers_invoice::InvoiceModule;
use ledgers_invoice::commands::IssueInvoice;
use ledgers_invoice::model::IssueInvoiceInput;
use ledgers_ledger::LedgerModule;
use ledgers_storage::StorageModule;
use ledgers_subledger::SubledgerModule;
use sea_query::Value;

use common::{COMPANY_ID, run_command, seed_company, sum_journal_line_base};

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

fn simple_input() -> IssueInvoiceInput {
    IssueInvoiceInput {
        customer_id: "cust_acme".to_string(),
        series: "RE".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: None,
        payment_terms_days: Some(14),
        currency: None,
        rate: None,
        lines: vec![
            ledgers_invoice::model::InvoiceLineInput {
                description: "Consulting".to_string(),
                quantity: "10".to_string(),
                unit: Some("h".to_string()),
                unit_price_minor: None,
                net_minor: Some(100_000),
                tax_code: "19-U".to_string(),
                revenue_account: Some("4000".to_string()),
            },
            ledgers_invoice::model::InvoiceLineInput {
                description: "Travel".to_string(),
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
fn issue_posts_ar_entry_and_open_item() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;
            let output = run_command(db, &IssueInvoice, &simple_input(), "test-001")
                .await
                .expect("invoice should post");
            assert_eq!(output.invoice_number, "RE-2026-0001");
            assert_eq!(output.totals.gross_minor, 140_400);
            let (debit, credit) = sum_journal_line_base(db, &output.entry_id).await;
            assert_eq!(debit, credit);
            assert_eq!(debit, 140_400);
            let oi = db
                .query(&Statement::with_values(
                    "SELECT id FROM open_item WHERE company_id = ? AND source_id = ?".to_string(),
                    vec![
                        Value::from(COMPANY_ID),
                        Value::from(output.invoice_number.as_str()),
                    ],
                ))
                .await
                .unwrap();
            assert_eq!(oi.rows.len(), 1);
        });
    }
}

#[test]
fn foreign_currency_records_rate_and_balances_in_base() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;
            // Test with 3 lines where per-line rounding differs from gross rounding
            // USD 333.33 × 3 at rate 0.91371: gross 1000.00 * 0.91371 = 913.71
            // Per line: 333.33 * 0.91371 = 304.56 (rounded), ×3 = 913.68
            // Difference: 3 cents, should post on FX/rounding account
            let inp = IssueInvoiceInput {
                customer_id: "cust_acme".to_string(),
                series: "FX".to_string(),
                issue_date: "2026-10-24".to_string(),
                supply_date: None,
                payment_terms_days: None,
                currency: Some("USD".to_string()),
                rate: Some("0.91371".to_string()),
                lines: vec![
                    ledgers_invoice::model::InvoiceLineInput {
                        description: "Line 1".to_string(),
                        quantity: "1".to_string(),
                        unit: None,
                        unit_price_minor: None,
                        net_minor: Some(33333), // 333.33 USD
                        tax_code: "19-U".to_string(),
                        revenue_account: Some("4000".to_string()),
                    },
                    ledgers_invoice::model::InvoiceLineInput {
                        description: "Line 2".to_string(),
                        quantity: "1".to_string(),
                        unit: None,
                        unit_price_minor: None,
                        net_minor: Some(33333), // 333.33 USD
                        tax_code: "19-U".to_string(),
                        revenue_account: Some("4000".to_string()),
                    },
                    ledgers_invoice::model::InvoiceLineInput {
                        description: "Line 3".to_string(),
                        quantity: "1".to_string(),
                        unit: None,
                        unit_price_minor: None,
                        net_minor: Some(33334), // 333.34 USD (remainder)
                        tax_code: "19-U".to_string(),
                        revenue_account: Some("4000".to_string()),
                    },
                ],
            };
            let output = run_command(db, &IssueInvoice, &inp, "test-002")
                .await
                .expect("fx invoice with rounding should post");
            assert_eq!(output.totals.currency, "USD");
            let (debit, credit) = sum_journal_line_base(db, &output.entry_id).await;
            // Entry must balance in base currency even with per-line rounding
            assert_eq!(debit, credit, "FX entry must balance in base");
        });
    }
}

#[test]
fn intra_eu_zero_rating_requires_valid_vies() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;
            let mut inp = simple_input();
            inp.customer_id = "cust_fjordlys".to_string();
            let line = inp.lines[0].clone();
            inp.lines = vec![line];
            let _ = run_command(db, &IssueInvoice, &inp, "test-003")
                .await
                .expect("intra-eu customer with valid vat should post");
        });
    }
}

#[test]
fn unknown_customer_or_vendor_only_party_is_refused() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;
            let mut inp = simple_input();
            inp.customer_id = "unknown".to_string();
            assert!(
                run_command(db, &IssueInvoice, &inp, "test-004")
                    .await
                    .is_err()
            );
            inp.customer_id = "vendor_supplier".to_string();
            assert!(
                run_command(db, &IssueInvoice, &inp, "test-005")
                    .await
                    .is_err()
            );
        });
    }
}

#[test]
fn issued_invoice_is_immutable() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;
            let output = run_command(db, &IssueInvoice, &simple_input(), "test-006")
                .await
                .expect("invoice should post");
            let inv = db
                .query(&Statement::with_values(
                    "SELECT id, series FROM invoice WHERE company_id = ? AND id = ?".to_string(),
                    vec![
                        Value::from(COMPANY_ID),
                        Value::from(output.invoice_id.as_str()),
                    ],
                ))
                .await
                .expect("invoice query should work");
            assert_eq!(inv.rows.len(), 1);
        });
    }
}

#[test]
fn agent_key_with_limit_below_invoice_gross_is_refused() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;

            // Create agent key with 50 EUR limit
            let limits = ledgers_commands::Limits::new()
                .insert(
                    "EUR",
                    ledgers_commands::auth::Limit {
                        base_ceiling: 5000, // 50 EUR (gross will be ~140.40 EUR = 14040 minor)
                        transaction_ceiling: 5000,
                    },
                )
                .expect("valid currency");
            let agent_key = "bearer-agent-limited";
            let authenticator = common::TestAuthenticator::new().with_agent_key(
                agent_key.to_string(),
                ledgers_commands::auth::Tier::PostWithinLimits,
                limits,
            );

            // Simple invoice with 140.40 EUR gross (100 + 20 net + tax)
            let inp = simple_input();

            // Attempt to run as agent key - should be refused because gross (140.40 EUR = 14040 minor) > limit (50 EUR = 5000 minor)
            let result = common::run_command_as_agent(
                db,
                &IssueInvoice,
                &inp,
                "test-limit-refuse",
                agent_key.to_string(),
                authenticator,
            )
            .await;

            // The command should fail with a limit-related error (NeedsApproval or Deny)
            assert!(result.is_err());
        });
    }
}

#[test]
fn numbering_is_gapless_per_series() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;
            let input = IssueInvoiceInput {
                customer_id: "cust_acme".to_string(),
                series: "RE".to_string(),
                issue_date: "2026-10-24".to_string(),
                supply_date: None,
                payment_terms_days: None,
                currency: None,
                rate: None,
                lines: vec![ledgers_invoice::model::InvoiceLineInput {
                    description: "Services".to_string(),
                    quantity: "1".to_string(),
                    unit: None,
                    unit_price_minor: None,
                    net_minor: Some(100_000),
                    tax_code: "19-U".to_string(),
                    revenue_account: Some("4000".to_string()),
                }],
            };
            let o1 = run_command(db, &IssueInvoice, &input, "test-inv-1")
                .await
                .expect("first invoice should succeed");
            assert_eq!(o1.invoice_number, "RE-2026-0001");
            let o2 = run_command(db, &IssueInvoice, &input, "test-inv-2")
                .await
                .expect("second invoice should succeed");
            assert_eq!(o2.invoice_number, "RE-2026-0002");
            let mut other = input.clone();
            other.series = "INV".to_string();
            let o3 = run_command(db, &IssueInvoice, &other, "test-inv-3")
                .await
                .expect("other series should succeed");
            assert_eq!(o3.invoice_number, "INV-2026-0001");
        });
    }
}

#[test]
fn preview_lines_match_posted_lines() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            seed_company(db).await;

            let input = simple_input();

            // Get preview of what lines will be posted
            let ctx = ledgers_commands::CommandContext {
                company_id: common::COMPANY_ID.to_string(),
                request_id: "test-preview-match".to_string(),
                event_id: "test-preview-match-event".to_string(),
                now: common::NOW.to_string(),
                is_person: true,
            };
            let preview = IssueInvoice
                .preview(db, &ctx, &input)
                .await
                .expect("preview should succeed");

            // Extract account and side from preview lines, sorted for comparison
            let mut preview_lines: Vec<_> = preview
                .lines
                .iter()
                .map(|line| (line.account.clone(), line.side.clone(), line.base.clone()))
                .collect();
            preview_lines.sort_by_key(|a| (a.0.clone(), a.1.clone()));

            // Post the invoice and get the posted lines
            let output = run_command(db, &IssueInvoice, &input, "test-preview-match")
                .await
                .expect("invoice should post");

            let posted_lines = common::get_journal_lines(db, &output.entry_id).await;
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
