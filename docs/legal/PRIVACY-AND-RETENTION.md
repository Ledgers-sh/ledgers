# Personal data and retention

**Draft, pending review (Ledgers-sh/ledgers#54). Not legal advice.**

Books contain personal data: sole traders as customers or vendors, contact
people, employees named on receipts, bank details. Two duties pull in
opposite directions:

- **Retention:** commercial and tax law require keeping books and their
  source documents (Germany: §147 AO and §257 HGB, up to 10 years).
- **Erasure:** data protection law gives people a right to erasure, which
  does not apply while a legal retention duty exists (GDPR Art. 17(3)(b)).

How Ledgers handles it:

1. Documents and entries linked to posted books are kept until their
   retention period ends, then become eligible for deletion.
2. Personal data that is not part of the books (unlinked uploads, contact
   details not needed for an entry, notification settings) can be erased on
   request at any time.
3. When the retention period ends, a person runs a reviewed purge that
   removes or anonymises personal data, recorded in the audit log without the
   erased values.
4. Exports with personal data are a human-only action.

Self-hosters are the controller of their data. In the hosted service, the
operator acts as processor under a data processing agreement
(Ledgers-sh/ledgers-cloud tracks this separately).
