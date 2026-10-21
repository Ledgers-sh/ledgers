# Extending the Ledgers core

Nine traits are extension points for customization at composition time.

## The dependency direction

The core (`Ledgers-sh/ledgers`, MIT) exposes extension traits with working
open-source defaults. See [ADR 0002](adr/0002-open-core-boundary.md).

## The seams

| Trait | Crate | Decides | Default |
|---|---|---|---|
| `EntityScope` | `ledgers-ext` | Which company a principal accesses. | `SingleCompany`: one company. |
| `UsageMeter` | `ledgers-ext` | Admit or pause actions. | `Unmetered`: always admit. |
| `DocumentIntake` | `ledgers-ext` | Where documents come from. | `UploadAndFolder`: HTTP + folder. |
| `BankFeed` | `ledgers-ext` | Pull transactions. | `StatementImportOnly`: import only. |
| `EInvoiceTransport` | `ledgers-ext` | Send/receive e-invoices. | `FileExchange`: export/import. |
| `Filing` | `ledgers-ext` | Submit tax returns. | `ManualFiling`: produce files. |
| `ApprovalRules` | `ledgers-approvals` | Who approves; step-up level. | `OwnerApprovesAll`: one owner. |
| `CloseCheck` | `ledgers-ledger` | Period-close blockers. | `DraftEntriesCloseCheck`. |
| `IdentityProvider` | `ledgers-auth` | Authenticate person sessions. | `OwnerPasskeys`: stub (fails closed). |

Each trait has rustdoc, an open default, and contract tests exported behind the
`testing` cargo feature.

## Composing a custom config

```rust
use ledgers_server::config::LedgersConfig;
use ledgers_ext::CompanyId;
use std::sync::Arc;

let config = LedgersConfig::open(CompanyId::new("company-001".to_string()));

// Replace one implementation
config.scope = Arc::new(MyEntityScope { /* ... */ });
```

## Reusing the contract tests

Enable `testing` features in dev-dependencies and call the contract test
function with your implementation:

```toml
[dev-dependencies]
ledgers-ext = { workspace = true, features = ["testing"] }
```

```rust
#[test]
fn my_usage_meter_satisfies_contract() {
    let meter = MyCustomMeter { /* ... */ };
    let company = CompanyId::new("test".to_string());
    ledgers_ext::meter::contract_tests::usage_meter(&meter, &company);
}
```

## Stability

Extension traits change only by deliberate ADR. Additive changes (new
non-exhaustive variants, optional traits) land in minor releases; breaking
changes require major releases.
