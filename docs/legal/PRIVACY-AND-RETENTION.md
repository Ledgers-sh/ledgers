# Personal data and retention

**Draft, pending review (Ledgers-sh/ledgers#54). Not legal advice.**

Books contain personal data: sole traders as customers or vendors, contact
people, employees named on receipts, bank details. Two duties pull in
opposite directions:

- **Retention:** commercial and tax law require keeping books and their
  source documents (Germany: §147 AO and §257 HGB, up to 10 years).
- **Erasure:** data protection law gives people a right to erasure, which
  does not apply while a legal retention duty exists (GDPR Art. 17(3)(b)).

## Where personal data lives today

1. **Approval requests** (crates/ledgers-approvals/migrations/sqlite/0001_approvals.sql):
   - `requested_by`, `decided_by`: identities
   - `preview`, `input`: journal preview with counterparty and amounts
2. **Bank transactions** (crates/ledgers-ext/src/bank.rs:12): `counterparty` field
3. **Audit log** (docs/audit-format.md:56–61): actor identities (`person_id`, `key_id`)
4. **API keys** (crates/ledgers-auth/src/keys.rs): via cratefield-core `ApiKeys` harness
5. **Planned:** parties (issue #44), documents (issues #20–23)

## How `cratefield-module-privacy` fits

Ledgers uses `cratefield-module-privacy` for subject access and erasure. Each
module declares tables via `Module::personal_data()`, compiled into a
`PersonalDataCatalog`; the privacy module calls `erase/plan` then
`erase/apply`, with erasure recorded in the audit log (no plaintext stored).
External systems (CRM, bank) register via `Privacy::provider()` (HTTP contract).

**Status:** Not wired in yet. Prerequisites: parties/documents modules (M2); Ledgers
uses cratefield-core =0.10.1, module-privacy requires "0.10" (compatible).

## How it works: retention until expiry, then anonymise

1. Documents and entries linked to posted books are kept until their
   retention period ends, then become eligible for deletion.
2. Personal data not part of the books (unlinked uploads, contact
   details not needed for an entry, notification settings) can be erased on
   request at any time.
3. When the retention period ends, a person runs a reviewed erasure plan from
   the privacy module, reviews what will be erased or anonymised per table,
   and applies it. The erasure is recorded in the audit log without storing
   the erased values (docs/audit-format.md: no plaintext in events).
4. Exports with personal data are a human-only action (policy enforced
   server-side: crates/ledgers-policy/src/lib.rs).

Self-hosters are the data controller. In the hosted service, the
operator acts as processor under a data processing agreement
(Ledgers-sh/ledgers-cloud tracks this separately).
