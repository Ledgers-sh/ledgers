# Users and permissions

Ledgers has one company, and the people who work in it hold one of six roles.
This page is the reference: who may do what, how people join, and what the
server enforces. The matrix itself lives in one place, `crates/ledgers-policy/src/roles.rs`;
the table below mirrors it, and its tests fail when the two disagree.

Decision record: [ADR 0002](adr/0002-open-core-boundary.md) (amended 2026-10-23),
which puts the fixed roles in the open core.

## Roles

| Role | For | What it may do |
|---|---|---|
| owner | the person who runs the company | everything; transfer ownership; add or remove any member |
| admin | people who run the books day to day | everything except changing or granting ownership |
| accountant | bookkeepers | read the books; post, reverse, bills, documents, accounts |
| approver | people who decide approval requests | read the books; decide approval requests |
| viewer | people who need to see the books | read the books and the approvals inbox |
| auditor | external or internal auditors | read the books, the audit log and exports |

Within the matrix, each command belongs to one class:

| Class | Commands | owner | admin | accountant | approver | viewer | auditor |
|---|---|:-:|:-:|:-:|:-:|:-:|:-:|
| read | accounts, entries, reports, documents, bills (read), parties (list, get, open items, export and import check), fiscal years, numbering, company, tax settings, groups and agreements (read), intercompany reconciliation and group view | yes | yes | yes | yes | yes | yes |
| settings | company update, tax settings update, approval settings, numbering sequence, party bank details (`parties.set_bank`, human only), party merge (`parties.merge`, human only), groups create and update (owner only, see below), intercompany agreements create, update and terminate | yes | yes | | | | |
| keys | agent keys (create, list, revoke), policy (put, revoke, explain) | yes | yes | | | | |
| audit | audit log (`audit-events`) | yes | yes | | | | yes |
| approvals-read | approvals inbox: list and get | yes | yes | | yes | yes | yes |
| approvals-decide | approve or reject a request | yes | yes | | yes | | |
| post | post, reverse, journal entries, bills create/update/extract/confirm/discard/post, documents register, accounts create/update/archive/restore, parties create/update, VAT check (`parties.check_vat`), logo (`parties.set_logo`) and CSV import (`parties.import`, bank rows need a person), intercompany periods run and post (`agreements.run_period`, `agreements.post_period`) | yes | yes | yes | | | |
| money-out | bill payments, intercompany settlement (`agreements.settle_period`) | yes | yes | | | | |
| close | close or reopen a period, create or close a fiscal year | yes | yes | | | | |

Party CSV files (`parties.export`, `parties.import_check`, `parties.import`)
guard against spreadsheet formulas. Party names can come from scanned invoices,
so an exported cell that starts with `=`, `+`, `-`, `@`, a tab, a carriage
return or an apostrophe gets one apostrophe in front (`=HYPERLINK(...)` exports
as `'=HYPERLINK(...)`, `'x` as `''x`, `-12.50` as `'-12.50`). An import removes
exactly one leading apostrophe, and only when the apostrophe is followed by one
of those characters or by another apostrophe; every other cell is read as it
is. The escape comes off before a cell is parsed, so a number reads as its
value. The round trip is exact: importing an export stores the same values.

Group commands touch more than one company, so the server checks the role in
every company a command names, not only the one in the request. A group
is created and its members changed by an owner of every member company.
Lists and reports show a person only the member companies they hold a role in;
a company they cannot see is never named, not even by its id.

A command that is not classified is refused for everyone, so a new command is
unusable until someone decides which role may run it.

Two decisions the spec did not spell out, recorded for review: an accountant
cannot close periods or pay bills (both are owner or admin), and an accountant
does not see the approvals inbox.

## Enforcement

- Every command runs through the executor, which asks the policy for each
  decision. A person's role is checked there, so HTTP, MCP and the CLI get the same answer.
- Routes that are not commands (agent keys, periods, fiscal years, tax settings,
  bill confirmation, approvals decisions) check the same matrix before they act.
- A refused command answers `403 policy-denied` and is recorded in the audit log as
  denied. The app hides or disables what the role cannot do, but the server
  decides.
- Approved requests: when an approver decides a request, the command the request
  was raised for runs under that approval. The approver's role is checked for the
  decision (`approvals-decide`), not again for the approved command. This is how an
  approver can approve an entry they could not post themselves.

## Who a person is

A person signs in through the company's Cratefield auth instance (see
[deploy.md](deploy.md), section 5a). Their membership in the company decides
their role. The `roles` claim the auth instance sends is read once, for the
bootstrap below, and never again: removing someone's membership takes effect on
their next request, not at their next sign-in.

### Bootstrap

The first sign-in whose token carries the `owner` role, while the company has
no owner, makes the company's organization and that person its owner. Nothing
else grants ownership at sign-in. Two sign-ins racing for the bootstrap leave
exactly one owner (one guarded batch). An `admin` claim does not bootstrap.

### A person with no membership

A signed-in person who holds no membership sees "You're not a member of this
company yet. Ask the owner for an invitation." Their session is valid but every
company route answers `403 not-a-member`.

## Invitations

- An owner or admin invites by email with a role. The invitation mail is sent by
  the organization module through Owlpost, from `ORGS_FROM`.
- The mail links to `<app>/accept?token=…` (`ORGS_ACCEPT_URL`). The token is
  single-use and lapses after seven days.
- Accepting needs a signed-in person whose verified address is the invited one.
  A person without an account signs in through the auth instance's Continue
  flow, which creates the account; then they accept.
- The company keeps only a hash of the address and of the token, so the pending
  list shows the role, who invited, and when it lapses, not the address.
- **Revoke** deletes an open invitation; its link stops working at once.
- **Resend** is revoke, then invite again with the address typed again. The
  company holds no plain address to resend to.

## Changing people

- Change a role: owners and admins. Only an owner may grant or change the owner role.
- Remove a member: owners and admins. Removing a member ends their access on
  their next request.
- Leave: any member, unless they are the last owner.
- **The company keeps at least one owner.** Removing, demoting or leaving as the
  last owner answers `409 orgs-last-owner`.
- **Transfer ownership:** promote the new owner to owner first, then demote yourself
  to admin. The order keeps an owner in place at every step.

## Companies

One deployment serves several companies. Each company has its own books,
chart, members and agent keys; the people who belong to one company never see
another one.

- `GET /v1/companies` lists the companies the signed-in person holds a
  membership in, with their role. `POST /v1/companies` creates one (name, base
  currency, country pack, fiscal year start month). The creator becomes its
  owner. Both need a person session; agent keys are refused. `GET
  /v1/companies/packs` lists the bundled country packs the create dialog offers.
  A person may create at most `LEDGERS_COMPANIES_PER_PERSON` companies (default
  10); past that the answer is `429 company-limit-reached`.
- Creating a company is atomic in effect: the company rows are written first,
  then the chart, journal and fiscal year. If the chart cannot be seeded, the
  company, organization, membership, settings, accounts, journal and fiscal
  year rows are removed again and the answer is `500 company-seed-failed`. The
  audit events of the attempt stay, because the audit chain is append-only.
- A company command names its company with the `X-Ledgers-Company` header
  (letters and digits, at most 64 characters). Without the header the request
  acts for the deployment's own company, `LEDGERS_COMPANY_ID`, as before.
- The membership check runs against the named company. A person who is not a
  member gets `403 not-a-member`; an agent key is bound to the company that
  issued it, so the same key named against another company gets `401`.
- The people routes (`/v1/auth/session`, `/people`, `/invitations`, `/roles`)
  serve the company the request names, with the membership check in that
  company.
- The `owner` sign-in claim bootstraps the deployment's own company only. A
  company created later gets its owner from its creator, never from a claim
  presented against it.
- The country pack chooses the starting chart: `nl` the RGS chart for a
  BV, `de` the HGB chart, any other pack the generic SME chart.
- The MCP endpoint serves the company its agent key belongs to, found from the
  key itself; the header is not needed there.
- A document download link names its company (`company=`) and the signature
  covers it. A link never serves another company: a request that names another
  company with the header gets `403`, and a link whose company was changed fails
  its signature.

## Agent keys

Agent keys belong to the company. Each key records the member who created it
(`created_by`). A key keeps working when that member leaves, is removed, or is
demoted: keys authenticate on their own credential and are not tied to the
membership. Only owners and admins create, list or revoke keys.

## Segregation of duties

A person may not approve a request that an agent key they created raised. The
server refuses with `403 segregation-of-duties`, and any other approver with the
approvals-decide class may decide it. Rejecting your own request is allowed.

Advanced rules (approval chains, thresholds per role, SoD policies beyond this
one) are part of the hosted service, not the open core.

## Member list and what it shows

The member list shows each member's address from their last sign-in, the role,
who invited them, when they joined and when they last used the app. The last-used
time moves when their session slides (about twice an hour while in use). The
company stores that address and time in `people_profile`, declared as the
person's own data: erasing the account removes the profile row. The membership
is removed with it, as the organization module declares for its member rows.

## Configuration

| Name | Where | Purpose |
|---|---|---|
| `ORGS_FROM` | `[env.staging.vars]` in `crates/ledgers-server/wrangler.toml` | the invitation sender, a `send.` address (staging: `invitations@send.ledgers.sh`) |
| `ORGS_ACCEPT_URL` | the same vars | the app's accept page (`https://app-staging.ledgers.sh/accept`) |
| `OWLPOST_API_KEY` | a Worker secret per environment: `wrangler secret put OWLPOST_API_KEY --env staging` | the Owlpost key. Without it invitations answer `503 mail-not-configured` |
| `OWLPOST_BASE_URL` | optional | a self-hosted or proxy Owlpost |

## Limits

- The display name is the address; the auth instance sends no name claim.
- The company has one organization. Creating a second is not offered
  (`404 org-create-not-offered`).
- Custom roles, SSO and SCIM are not in the open core.
