# 0002: Open core in this repository, paid features in ledgers-cloud

Date: 2026-10-09. Status: accepted.

## Decision

Two repositories. `Ledgers-sh/ledgers` (MIT) holds everything one company
needs to keep correct books: every accounting module, 3-way matching,
e-invoice formats, bank statement import, tax calculation, reports, audit,
export, policy, single-approver passkey approvals, CLI, MCP, API, the app and
self-hosting. `Ledgers-sh/ledgers-cloud` (proprietary, private) holds what is
about more people (roles, approval rules, segregation of duties, SSO), more
companies (groups, intercompany, consolidation) or running costs (tenancy,
billing, quotas, hosted agents, bank-feed aggregators, e-invoice networks,
filing gateways, uptime).

The dependency is one way: cloud depends on core. Core exposes extension
points as traits with a working open default (`ApprovalRules`, `EntityScope`,
`UsageMeter`, `DocumentIntake`, `BankFeed`, `EInvoiceTransport`, `Filing`);
cloud provides implementations at composition time.

## Consequences

- A CI check here fails on any reference to a `ledgers-cloud` crate or path.
- A feature moves from cloud to core only by deliberate decision recorded here;
  core features never move to cloud.
- The site's pricing says the free tier is the full core for one company
  (Ledgers-sh/website#3).
