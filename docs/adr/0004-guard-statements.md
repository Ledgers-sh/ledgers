# 0004: Guard statements — write-time invariants on `batch_atomic`

Date: 2026-10-19. Status: accepted.

## Context

The harness `Database` port offers `execute`, `query` and `batch_atomic`
only: no interactive transactions, and a batch reports no row counts. So
nothing can "read, check, then write" inside one transaction — the
read and the check would sit outside the write's atomicity. The
correctness rules (docs/PLAN.md §3) nevertheless demand checks at write
time: gapless numbering, balanced entries, closed periods rejecting
postings, append-only audit chain, agent limits. The harness's own
`usage::Usage::consume_statement` already shows the shape: a statement
that **fails** when the invariant is violated.

## Decision

Every write-time check is a **guard statement** inside the posting
batch. The general form (ledgers-db `guard::assert_statement`):

```sql
INSERT INTO ledger_guard (guard_key, ok)
SELECT ?, CASE WHEN <condition> THEN 1 ELSE NULL END
```

`ok` is `NOT NULL`, so the insert aborts — and with it the whole
`batch_atomic` batch — unless `<condition>` is true. Unknown (NULL)
fails too, so a missing row fails closed. Two worked instances:

```sql
-- closed period: refuses unless the period row says 'open'
INSERT INTO ledger_guard (guard_key, ok)
SELECT ?, CASE WHEN (SELECT status FROM accounting_period
                     WHERE company_id = ? AND period = ?) = 'open'
        THEN 1 ELSE NULL END;

-- balance, defence in depth behind the domain check:
-- the lines just inserted must sum to zero in one currency
INSERT INTO ledger_guard (guard_key, ok)
SELECT ?, CASE WHEN (SELECT COUNT(DISTINCT currency) FROM demo_line
                     WHERE entry_id = ?) = 1
             AND (SELECT COALESCE(SUM(amount_minor), 0) FROM demo_line
                  WHERE entry_id = ?) = 0
        THEN 1 ELSE NULL END;
```

Guard probe rows are batch-local scratch: the plan deletes them
(`guard::release_statement`) before the batch commits.

The patterns around them, all in `ledgers-db`:

- **Gapless numbering** — the plan reads the counter before the batch
  (`read_counter`), computing the expected number — needed pre-batch
  as it forms the audit event's hashed input, letting `post_entry`
  return it without a post-commit read. Inside the batch, provisions
  the counter row (idempotent upsert), bumps it (the `UPDATE` takes the
  row lock that serialises concurrent posters), inserts the entry with
  that literal expected number, and verifies the counter advanced with a
  guard statement. A race that lost rereads the counter and retries with
  a fresh batch. `UNIQUE (journal_id, fiscal_year, number)` is the
  backstop; the guard is the enforcer. The entry number is immutable on
  the journal_entry row; the audit event references the entry id and the
  number binds via input hash, so no stored number field is needed in the
  audit event.
- **Audit chain** — the plan reads the head, appends with
  `seq = head.seq + 1`; `UNIQUE (company_id, seq)` makes a fork a
  constraint failure, never a stored fork.
- **Agent limits** — the harness `Usage` guarded upsert, verbatim:
  admission and increment are one statement, and a spent allowance is a
  `NOT NULL` failure.

**Retry policy** (`retry::run_batch_with_retry`): exactly one failure
class is legal to retry — a **unique violation**, a race the batch lost
fairly (stale chain head, numbering backstop). Each attempt re-plans
the batch from fresh reads; the sleep is an injected `FnMut(Duration)`
exponential ladder (`RetryPolicy::default`: 8 attempts, 10 ms doubling
to 500 ms). A `NOT NULL` refusal is **final** — re-running a guard
re-asks the same question, and retrying a policy refusal would turn it
into a timing attack on the limit or the period. Exhausted retries
surface as `GuardError::Conflict` (contention), never as a refusal.

## Consequences

- One crate owns the patterns; every writing crate composes the same
  statements, and the proofs live in `ledgers-db`'s test matrix
  (`tests/matrix.rs`, `tests/concurrency.rs`).
- The 1 000-post proof (100 threads, one journal, SQLite and Postgres)
  commits every post with numbers exactly `1..=1000` and an unforked
  chain; the limit proof commits exactly 200 of 1 000 and refuses the
  rest as `NOT NULL`, never as conflict. A post whose retry budget
  exhausts is resubmitted by the test exactly as a caller would on
  `Conflict`, after checking it left no entry, line or audit event;
  the meter and the numbering then count only committed posts, which
  proves an exhausted batch spent no allowance and burned no number.
- Two measured findings shape deployments: retries need **jitter**
  (a plain ladder keeps a conflicted herd synchronised; the jitter
  belongs in the injected sleeper), and the **budget must fit the
  contention** — on Postgres the numbering row lock serialises posters
  (~85 commits/s in the test setup), and the 8-attempt default stranded
  ~200 of 1 000 posts under a 100-thread herd, while a 40-attempt
  budget commits all of them. `RetryPolicy` is data; size it for the
  herd you actually run.
- **Exhaustion is a legitimate outcome, not a defect.** No attempt
  budget outlasts every machine: on Postgres each attempt reads the
  chain head and submits its batch through two separate pool
  checkouts, so under a herd the head is usually stale by the time the
  batch runs and the attempts a post needs scale with server speed.
  The first CI run of the Postgres leg drained in 18.9 s (vs ~2.7 s on
  a workstation) and one post of 1 000 exhausted a 40-attempt budget.
  What the design guarantees is that exhaustion is a clean, retryable
  `GuardError::Conflict` — nothing written, no number gap — and that
  is what the proofs assert; "zero exhaustions" would assert the
  runner's speed. Contention that large on one company chain is a
  deployment shape to avoid (serialise writers per company upstream),
  not one to absorb with an ever larger budget.
- The library never sleeps or blocks itself (injected sleep keeps it
  wasm-clean for the Workers runtime).
