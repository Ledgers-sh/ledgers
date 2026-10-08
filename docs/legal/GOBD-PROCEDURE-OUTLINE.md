# GoBD procedure documentation (Verfahrensdokumentation): outline

**Draft outline for German users, pending review (Ledgers-sh/ledgers#54).**

German businesses must document how their bookkeeping works (GoBD, BMF
letter of 28 November 2019, as amended). Ledgers will ship a template that
users fill in. Planned chapters:

1. General description: company, who keeps the books, which Ledgers version
   and deployment (self-hosted or managed).
2. User documentation: how documents arrive, how agents extract and propose,
   how people review, approve and post.
3. Technical system documentation: storage, backups, the audit log and its
   hash chain, immutability of posted entries, gapless numbering.
4. Operating documentation: agent keys and their limits, approval rules,
   month-end and year-end close, access control.
5. Internal control system: policy tiers, human-only actions, passkey
   approvals, segregation of duties where used.
6. Data access for tax auditors (Z1/Z2/Z3) and the export format.
7. Retention and deletion.
