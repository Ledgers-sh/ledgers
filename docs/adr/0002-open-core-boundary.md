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
`UsageMeter`, `DocumentIntake`, `BankFeed`, `EInvoiceTransport`, `Filing`,
`IdentityProvider`, `CloseCheck`);
cloud provides implementations at composition time.

## Consequences

- A CI check here fails on any reference to a `ledgers-cloud` crate or path.
- A feature moves from cloud to core only by deliberate decision recorded here;
  core features never move to cloud.
- The site's pricing says the free tier is the full core for one company
  (Ledgers-sh/website#3).

## Amendment 2026-10-23: the fixed roles are open core

The first decision above put "more people (roles, approval rules, SSO)" in the
cloud. The owner has asked for multi-person access in the open build, so this
amendment moves one part of that line and says exactly which part.

**Moves to core.** The six fixed roles: owner, admin, accountant, approver,
viewer and auditor. With them, the open build has the company's membership
(one organization from `cratefield-module-orgs`), invitations by email, role
changes, removal, the last-owner rule, ownership transfer, the permission
matrix enforced on every command, and segregation of duties on approvals (a
person cannot approve a request raised through an agent key they created). See
[docs/permissions.md](../permissions.md).

**Stays in the cloud.** Custom roles beyond the six; approval rules, chains and
thresholds per role; segregation-of-duties policies beyond the one above; SSO and
SCIM provisioning. The extension points that carry them (`ApprovalRules`, and
the matrix's role source) keep their open defaults in core, so the cloud supplies
implementations at composition time as before.

**Why.** Roles are the thing the owner needs to let a bookkeeper or an auditor
in without sharing the owner's sign-in. Refusing that in the open build made a
single-company ledger unusable for any company with more than one person. The
boundary above keeps what is about running a service for many companies, and
what needs policy machinery, in the cloud.

**Unchanged.** The dependency still points one way; no crate here names a cloud
crate (`tools/check-boundary.sh`). Section 5 of the plan ("the open-source build
has one approver (the owner)") is superseded for the fixed roles: any member with
the approvals-decide class may decide a request, other than the person who
raised it.

