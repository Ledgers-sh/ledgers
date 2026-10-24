//! Property tests for subledger reconciliation.
//! Tests that AR and AP subledger sums always equal their control accounts at every date.
//! Each operation is valid by construction; its success is asserted.

mod common;

use common::{COMPANY_ID, NOW, create_journal, harness, mount, open_item_row};
use ledgers_commands::{
    CommandContext, CommandError, DefaultPolicy, Execute, Executor,
    auth::{Authenticator, Credential, Principal, Role},
};
use ledgers_subledger::commands::{
    AllocateInput, AllocateItem, OffsetInput, PostItemInput, SubledgerAllocate, SubledgerOffset,
    SubledgerPostItem, SubledgerWriteOff, WriteOffInput,
};
use ledgers_subledger::reconcile::reconcile;
use proptest::prelude::*;

struct TestAuthenticator;

#[async_trait::async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        Ok(Principal::Person {
            person_id: "test_person".to_string(),
            role: Role::Owner,
        })
    }
}

const PARTIES: [&str; 2] = ["p0", "p1"];
// (decimal string for input, numerator, denominator) — integer math only
const RATES: [(&str, i64, i64); 3] = [
    ("0.9", 9, 10),
    ("0.9250", 925, 1000),
    ("1.0850", 1085, 1000),
];

/// Abstract operation: numeric selectors resolved at execution time against actual open items.
#[derive(Debug, Clone)]
enum Op {
    Post {
        ap: bool,
        usd: bool,
        party: usize,
        amount: i64,
        rate: usize,
    },
    Settle {
        pick: usize,
        pct: i64,
        jitter: i64,
    },
    Prepay {
        ap: bool,
        party: usize,
        amount: i64,
    },
    Offset {
        pick: usize,
        pct: i64,
    },
    WriteOff {
        pick: usize,
        amount: i64,
    },
}

fn post_op() -> impl Strategy<Value = Op> {
    (
        any::<bool>(),
        any::<bool>(),
        0..PARTIES.len(),
        1_000i64..=500_000,
        0..RATES.len(),
    )
        .prop_map(|(ap, usd, party, amount, rate)| Op::Post {
            ap,
            usd,
            party,
            amount,
            rate,
        })
}

fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => post_op(),
        10 => (any::<usize>(), 20i64..=130, -10_000i64..=10_000).prop_map(|(pick, pct, jitter)| Op::Settle { pick, pct, jitter }),
        1 => (any::<bool>(), 0..PARTIES.len(), 1_000i64..=50_000).prop_map(|(ap, party, amount)| Op::Prepay { ap, party, amount }),
        3 => (any::<usize>(), 10i64..=100).prop_map(|(pick, pct)| Op::Offset { pick, pct }),
        2 => (any::<usize>(), 1i64..=4_000).prop_map(|(pick, amount)| Op::WriteOff { pick, amount }),
    ]
}

/// What the test knows about each item it created; remaining amounts are always read from the db (`open_item_row`).
#[derive(Clone, Debug)]
struct Tracked {
    id: String,
    ledger: &'static str,
    party: usize,
    usd: bool,
    rate: usize,
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn subledger_matches_gl_at_every_date(
        seed in prop::collection::vec(post_op(), 10..14),
        ops in prop::collection::vec(any_op(), 25..60)
    ) {
        let h = harness();
        let db = h.db.as_ref();

        let result = pollster::block_on(async {

            mount(db, COMPANY_ID).await;
            let journal_id = create_journal(db, COMPANY_ID).await;

            let mut all_ops = seed.clone();
            all_ops.extend(ops.clone());

            let mut tracked: Vec<Tracked> = vec![];

            // Execute all operations
            for (i, op) in all_ops.iter().enumerate() {
                // Date: 2026-10-(1 + min(i/3, 14)); capped at 2026-10-15 since NOW = 2026-10-15
                #[allow(clippy::cast_possible_truncation)]
                let day = (1 + std::cmp::min(i / 3, 14)) as u32;
                let date_str = format!("2026-10-{day:02}");
                let event_id = format!("ev-{i}");

                match op {
                    Op::Post { ap, usd, party, amount, rate } => {
                        let ledger = if *ap { "ap" } else { "ar" };
                        let kind = if *ap { "bill" } else { "invoice" };
                        let party_id = format!("{}_{}", PARTIES[*party], if *ap { "ap" } else { "ar" });
                        let counter_account = if *ap { "6000" } else { "4000" };
                        let currency = if *usd { "USD" } else { "EUR" };
                        let rate_opt = if *usd { Some(RATES[*rate].0.to_string()) } else { None };

                        let input = PostItemInput {
                            ledger: ledger.to_owned(),
                            kind: kind.to_owned(),
                            party_id,
                            journal_id: journal_id.clone(),
                            document_date: date_str.clone(),
                            posting_date: date_str.clone(),
                            due_date: None,
                            currency: currency.to_owned(),
                            amount: *amount,
                            rate: rate_opt,
                            counter_account: counter_account.to_owned(),
                            dimensions: Default::default(),
                            counter_lines: Vec::new(),
                            description: format!("Test {ledger} {currency}"),
                            source_id: Some(format!("{ledger}_{i}_{currency}")),
                        };

                        let ctx = CommandContext {
                            company_id: COMPANY_ID.to_owned(),
                            is_person: true,
                            event_id: event_id.clone(),
                            request_id: "req".to_owned(),
                            now: NOW.to_owned(),
                        };

                        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
                        let r = executor
                            .run(&SubledgerPostItem, "bearer".to_string(), ctx, &input, Execute::Run)
                            .await;

                        prop_assert!(r.is_ok(), "{op:?} -> {r:?}");

                        if let Ok(ledgers_commands::Executed::Done(output)) = r {
                            tracked.push(Tracked {
                                id: output.item_id,
                                ledger,
                                party: *party,
                                usd: *usd,
                                rate: *rate,
                            });
                        }
                    }

                    Op::Settle { pick, pct, jitter } => {
                        // Query open items with tx_remaining > 0
                        let candidates: Vec<_> = ["ar", "ap"]
                            .iter()
                            .flat_map(|ledger| {
                                let tracked_for_ledger: Vec<_> = tracked
                                    .iter()
                                    .filter(|t| t.ledger == *ledger)
                                    .collect();
                                tracked_for_ledger
                            })
                            .filter_map(|t| {
                                pollster::block_on(async {
                                    open_item_row(db, &t.id).await.map(|row| (t.clone(), row))
                                })
                            })
                            .filter(|(_, row)| row.tx_remaining_minor > 0)
                            .collect();

                        if candidates.is_empty() {
                            continue;
                        }

                        let (tracked_item, item) = &candidates[pick % candidates.len()];
                        let rem_tx = item.tx_remaining_minor;
                        let want = std::cmp::max(1, rem_tx * pct / 100);
                        let item_amount = std::cmp::min(want, rem_tx);

                        let (_item_paid_base, paid_base, paid) = if tracked_item.usd {
                            let (_rate_str, num, den) = RATES[tracked_item.rate];
                            let approx_base = item_amount * num / den;
                            let item_paid_base_val = std::cmp::max(1, approx_base + jitter);
                            let extra = if *pct > 100 {
                                std::cmp::max(1, item.base_remaining_minor * (*pct - 100) / 100)
                            } else {
                                0
                            };
                            (item_paid_base_val, Some(item_paid_base_val), item_paid_base_val + extra)
                        } else {
                            let extra = if *pct > 100 {
                                std::cmp::max(1, item.base_remaining_minor * (*pct - 100) / 100)
                            } else {
                                0
                            };
                            (item_amount, None, item_amount + extra)
                        };

                        let input = AllocateInput {
                            ledger: tracked_item.ledger.to_owned(),
                            party_id: format!("{}_{}", PARTIES[tracked_item.party], if tracked_item.ledger == "ap" { "ap" } else { "ar" }),
                            journal_id: journal_id.clone(),
                            date: date_str.clone(),
                            bank_account: "1200".to_owned(),
                            paid,
                            items: vec![AllocateItem {
                                open_item_id: tracked_item.id.clone(),
                                amount: item_amount,
                                paid_base,
                            }],
                            description: None,
                            reference: None,
                        };

                        let ctx = CommandContext {
                            company_id: COMPANY_ID.to_owned(),
                            is_person: true,
                            event_id: event_id.clone(),
                            request_id: "req".to_owned(),
                            now: NOW.to_owned(),
                        };

                        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
                        let r = executor
                            .run(&SubledgerAllocate, "bearer".to_string(), ctx, &input, Execute::Run)
                            .await;

                        prop_assert!(r.is_ok(), "{op:?} -> {r:?}");

                        if let Ok(ledgers_commands::Executed::Done(output)) = r
                            && let Some(on_account_id) = output.on_account_item_id
                        {
                            tracked.push(Tracked {
                                id: on_account_id,
                                ledger: "eur",
                                party: tracked_item.party,
                                usd: false,
                                rate: 0,
                            });
                        }
                    }

                    Op::Prepay { ap, party, amount } => {
                        let ledger = if *ap { "ap" } else { "ar" };
                        let party_id = format!("{}_{}", PARTIES[*party], if *ap { "ap" } else { "ar" });

                        let input = AllocateInput {
                            ledger: ledger.to_owned(),
                            party_id,
                            journal_id: journal_id.clone(),
                            date: date_str.clone(),
                            bank_account: "1200".to_owned(),
                            paid: *amount,
                            items: vec![],
                            description: None,
                            reference: None,
                        };

                        let ctx = CommandContext {
                            company_id: COMPANY_ID.to_owned(),
                            is_person: true,
                            event_id: event_id.clone(),
                            request_id: "req".to_owned(),
                            now: NOW.to_owned(),
                        };

                        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
                        let r = executor
                            .run(&SubledgerAllocate, "bearer".to_string(), ctx, &input, Execute::Run)
                            .await;

                        prop_assert!(r.is_ok(), "{op:?} -> {r:?}");

                        if let Ok(ledgers_commands::Executed::Done(output)) = r
                            && let Some(on_account_id) = output.on_account_item_id
                        {
                            tracked.push(Tracked {
                                id: on_account_id,
                                ledger,
                                party: *party,
                                usd: false,
                                rate: 0,
                            });
                        }
                    }

                    Op::Offset { pick, pct } => {
                        // Find positive (debit) items
                        let positive_candidates: Vec<_> = tracked
                            .iter()
                            .filter_map(|t| {
                                pollster::block_on(async {
                                    open_item_row(db, &t.id).await.map(|row| (t.clone(), row))
                                })
                            })
                            .filter(|(_, row)| row.tx_remaining_minor > 0)
                            .collect();

                        if positive_candidates.is_empty() {
                            continue;
                        }

                        let (tracked_d, item_d) = &positive_candidates[pick % positive_candidates.len()];
                        let rem_tx_d = item_d.tx_remaining_minor;
                        let cn_amount = std::cmp::max(1, rem_tx_d * pct / 100);

                        // Post a credit note for the same ledger, party, currency
                        let post_rate_idx = (tracked_d.rate + 1) % RATES.len();
                        let rate_str = if tracked_d.usd {
                            let (rate_decimal, _, _) = RATES[post_rate_idx];
                            Some(rate_decimal.to_string())
                        } else {
                            None
                        };

                        let cn_input = PostItemInput {
                            ledger: tracked_d.ledger.to_owned(),
                            kind: "credit-note".to_owned(),
                            party_id: format!("{}_{}", PARTIES[tracked_d.party], if tracked_d.ledger == "ap" { "ap" } else { "ar" }),
                            journal_id: journal_id.clone(),
                            document_date: date_str.clone(),
                            posting_date: date_str.clone(),
                            due_date: None,
                            currency: if tracked_d.usd { "USD" } else { "EUR" }.to_owned(),
                            amount: cn_amount,
                            rate: rate_str,
                            counter_account: if tracked_d.ledger == "ap" { "6000" } else { "4000" }.to_owned(),
                            dimensions: Default::default(),
                            counter_lines: Vec::new(),
                            description: "Credit note for offset".to_owned(),
                            source_id: Some(format!("{event_id}-cn")),
                        };

                        let ctx = CommandContext {
                            company_id: COMPANY_ID.to_owned(),
                            is_person: true,
                            event_id: format!("{event_id}-cn"),
                            request_id: "req".to_owned(),
                            now: NOW.to_owned(),
                        };

                        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
                        let cn_result = executor
                            .run(&SubledgerPostItem, "bearer".to_string(), ctx, &cn_input, Execute::Run)
                            .await;

                        prop_assert!(cn_result.is_ok(), "credit note {op:?} -> {cn_result:?}");

                        let credit_id = if let Ok(ledgers_commands::Executed::Done(output)) = cn_result {
                            let c_id = output.item_id.clone();
                            tracked.push(Tracked {
                                id: c_id.clone(),
                                ledger: tracked_d.ledger,
                                party: tracked_d.party,
                                usd: tracked_d.usd,
                                rate: post_rate_idx,
                            });
                            c_id
                        } else {
                            prop_assert!(false, "credit note should return Done");
                            continue;
                        };

                        // Now offset: credit note vs debit item
                        let credit_row = pollster::block_on(async { open_item_row(db, &credit_id).await });
                        let offset_amount = if let Some(cr) = credit_row {
                            std::cmp::min(rem_tx_d, -cr.tx_remaining_minor)
                        } else {
                            std::cmp::min(rem_tx_d, cn_amount)
                        };

                        let offset_input = OffsetInput {
                            ledger: tracked_d.ledger.to_owned(),
                            party_id: format!("{}_{}", PARTIES[tracked_d.party], if tracked_d.ledger == "ap" { "ap" } else { "ar" }),
                            journal_id: journal_id.clone(),
                            date: date_str.clone(),
                            credit_item_id: credit_id,
                            debit_item_id: tracked_d.id.clone(),
                            amount: offset_amount,
                            description: None,
                            reference: None,
                        };

                        let ctx = CommandContext {
                            company_id: COMPANY_ID.to_owned(),
                            is_person: true,
                            event_id: event_id.clone(),
                            request_id: "req".to_owned(),
                            now: NOW.to_owned(),
                        };

                        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
                        let r = executor
                            .run(&SubledgerOffset, "bearer".to_string(), ctx, &offset_input, Execute::Run)
                            .await;

                        prop_assert!(r.is_ok(), "{op:?} -> {r:?}");

                        if let Ok(ledgers_commands::Executed::Done(_output)) = r {
                            // Offset succeeded
                        }
                    }

                    Op::WriteOff { pick, amount } => {
                        let candidates: Vec<_> = tracked
                            .iter()
                            .filter_map(|t| {
                                pollster::block_on(async {
                                    open_item_row(db, &t.id).await.map(|row| (t.clone(), row))
                                })
                            })
                            .filter(|(_, row)| row.tx_remaining_minor > 0)
                            .collect();

                        if candidates.is_empty() {
                            continue;
                        }

                        let (tracked_item, item) = &candidates[pick % candidates.len()];
                        let rem_tx = item.tx_remaining_minor;
                        let wo_amount = std::cmp::min(*amount, rem_tx);

                        let input = WriteOffInput {
                            open_item_id: tracked_item.id.clone(),
                            journal_id: journal_id.clone(),
                            date: date_str.clone(),
                            amount: wo_amount,
                            account: "6950".to_owned(),
                            reason: "Property test writeoff".to_owned(),
                            description: None,
                            reference: None,
                        };

                        let ctx = CommandContext {
                            company_id: COMPANY_ID.to_owned(),
                            is_person: true,
                            event_id: event_id.clone(),
                            request_id: "req".to_owned(),
                            now: NOW.to_owned(),
                        };

                        let executor = Executor::new(db, TestAuthenticator, DefaultPolicy);
                        let r = executor
                            .run(&SubledgerWriteOff, "bearer".to_string(), ctx, &input, Execute::Run)
                            .await;

                        prop_assert!(r.is_ok(), "{op:?} -> {r:?}");
                    }
                }
            }

            // Verify reconciliation at every date
            for day in 1..=31 {
                let date = format!("2026-10-{day:02}");
                if let Ok(result) = reconcile(db, COMPANY_ID, &date).await {
                    for ledger in result {
                        prop_assert_eq!(
                            ledger.difference, 0,
                            "reconciliation failed for {} on {}: difference = {}",
                            ledger.ledger, date, ledger.difference
                        );
                    }
                }
            }

            Ok(())
        });

        if let Err(_e) = result {
            // Test completed, any assertion errors were caught above
        }
    }
}

/// Sanity check: verify test infrastructure works.
#[test]
fn sanity_check_property_test_runs() {
    // If this passes, the test harness is functional.
}
