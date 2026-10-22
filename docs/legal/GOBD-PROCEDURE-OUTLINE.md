# GoBD procedure documentation (Verfahrensdokumentation): template

**Draft template for German users, pending review (Ledgers-sh/ledgers#54).**

German businesses must document their bookkeeping procedures (GoBD,
Grundsätze zur Ordnungsmäßigkeit der Buchführung und Aufzeichnungen mit
Datenverarbeitungsanlagen, BMF letter of 28 November 2019, as amended).
Fill in the sections below that apply to your use of Ledgers. Sections marked
as pre-filled describe what Ledgers itself enforces; verify these match your
deployment and add your own safeguards where noted.

## 1. General description (Allgemeine Beschreibung) — Rz. 151

- **Company:** [fill in: legal name, registered office, tax ID]
- **Fiscal year:** [fill in: 1 Jan – 31 Dec / other period]
- **Bookkeeping responsibility:** [fill in: name and title of person(s) responsible
  for the accounts]
- **Ledgers version:** [fill in: version in use; check `/v1/health` or `ledgers --version`]
- **Deployment:** [fill in: self-hosted (URL), or managed service]
- **Ledgers guarantees:**
  - Every posted entry is immutable; corrections are reversals (crates/ledgers-ledger/src/posting.rs:30–33).
  - Journal entry numbering is gapless per journal and fiscal year (crates/ledgers-db/src/numbering.rs).
  - All actions are recorded in an append-only, hash-chained audit log (docs/audit-format.md).

## 2. User documentation (Anwenderdokumentation) — Rz. 152

Describe how documents and transactions enter Ledgers:

- **Document intake:** [fill in: how invoices, receipts and bank statements arrive
  — uploads, email, bank feeds, API]
- **Agent role:** [fill in: which agents are used, what they extract and propose
  (e.g., invoice-to-entry drafting)]
- **Review and approval:** [fill in: how and when a person reviews agent
  proposals before posting; who has approval authority]
- **Posting:** [fill in: who posts entries, and how (API, web UI, CLI)]
- **Ledgers guarantees:**
  - Policy is enforced on the server for every command; agents do not decide what they may post (crates/ledgers-policy/src/lib.rs).
  - High-risk approvals require passkey assertion, preventing replay (crates/ledgers-approvals/src/lib.rs).

## 3. Technical system documentation (Technische Systemdokumentation) — Rz. 153

- **Storage:** [fill in: SQLite file location / Postgres database and credentials]
- **Backup:** [fill in: backup frequency and location]
- **Ledgers guarantees:**
  - Hash-chained audit log, immutable once posted: SHA-256(prev_hash || canonical_json(event)) per docs/audit-format.md.
  - Audit log exports with every posting, verifiable offline; the export is canonical JSON Lines, one event per line.
  - Numbering guard: the counter row's lock inside a batch serialises concurrent postings; a rolled-back posting leaves no gap (crates/ledgers-db/src/numbering.rs).
  - Schema migrations are forward-only, numbered, never edited once shipped; a migration never updates or deletes posted entries or audit events (crates/ledgers-ledger/migrations/sqlite/).

## 4. Operating documentation (Betriebsdokumentation) — Rz. 154

- **Agent keys:** [fill in: which agents exist, their tier (`read`, `draft`, `post-within-limits`), per-currency limits, creation date, revocation status]
- **Human access:** [fill in: who may approve, who may close periods, who may change policy]
- **Period close:** [fill in: month-end and year-end procedures; who performs close and reopen]
- **Ledgers guarantees:**
  - Closed periods reject postings; missing period reads as closed (crates/ledgers-ledger/src/period.rs).
  - Policy decisions (allow, needs-approval, deny) run on the server before any write (crates/ledgers-policy/src/lib.rs).
  - Only a person may close or reopen a period; agents cannot (crates/ledgers-ledger/src/period.rs:28–29).

## 5. Internal control system (IKS) — Rz. 155

- **Segregation of duties:** [fill in: which roles exist and what each may do; if roles overlap, describe compensating controls]
- **Key tier policy:**
  - `read`: [fill in: agents that only read, e.g. reporting or audit]
  - `draft`: [fill in: agents that draft entries but never post, e.g. extraction]
  - `post-within-limits`: [fill in: agents that post up to daily/entry limits per currency]
- **Human-only actions:** [fill in: confirmation that paying money out, closing periods, filing tax and changing policy are never delegated to agents]
- **Passkey approval:** [fill in: whether passkeys are in use; if so, describe who holds credentials and how recovery works]
- **Ledgers guarantees:**
  - Agent keys carry a tier and per-currency limits; limits are charged daily and per entry (crates/ledgers-policy/src/lib.rs).
  - Paying money, closing periods, filing, and policy changes are marked `HumanOnly` in the command registry and rejected for agents (crates/ledgers-commands/src/registry.rs).

## 6. Data access for tax audits (Datenzugriff) — Z1, Z2, Z3

- **Format:** [fill in: GoBD/DATEV export (planned: crates/ledgers-export)]
- **Completeness:** Ledgers exports the complete audit log (docs/audit-format.md, seq 1 onwards),
  all posted entries with their hash-chained proofs, all journals and accounts.
- **Verifiability:** An offline verifier can check the hash chain without Ledgers software
  (docs/audit-format.md, canonical JSON RFC 8785, SHA-256 chains).
- **Retention check:** [fill in: audit log and documents from fiscal year X are retained until [date];
  the retention period is [fill in: Germany is 10 years per §147 AO / §257 HGB]]

## 7. Retention and deletion (Aufbewahrung) — Rz. 151–152 revisited

Books and source documents must be kept for [fill in: Germany: 10 years from end
of fiscal year]. Personal data not part of the books may be erased earlier
(see docs/legal/PRIVACY-AND-RETENTION.md).

- **Posted entries:** kept for the full retention period; never deleted or updated.
- **Reversals:** a reversal is a new entry posted to the same journal, never a deletion.
- **Deletion after expiry:** [fill in: procedure for reviewing and purging or anonymising personal
  data after the retention period ends, with audit log entry recording the action]
