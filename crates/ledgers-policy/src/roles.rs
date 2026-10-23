//! The permission matrix: which company role may run which command.
//!
//! This is the one place the role rules live. A person's role comes from
//! their membership in the company organization (docs/permissions.md). Every
//! command is put in one [`Class`]; [`allows`] says which classes a role may
//! run. [`check`] is what the policy engine calls for every person principal,
//! on every surface (HTTP, MCP, CLI), so the server enforces the matrix and the
//! app only hides what the matrix refuses.
//!
//! A command whose name is not classified is refused for everyone: a new
//! command is unusable until someone decides which role may run it.

use ledgers_commands::Role;

/// What a command does, as far as the roles are concerned.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Class {
    /// Reads of the books: accounts, entries, reports, documents, bills,
    /// fiscal years, numbering, the company and the tax settings.
    Read,
    /// Changes the company's settings: company, tax, approval rules,
    /// numbering sequence.
    Settings,
    /// Agent keys and the policy that governs them.
    Keys,
    /// Reading the audit log.
    Audit,
    /// Reading the approvals inbox and its requests.
    ApprovalsRead,
    /// Approving or rejecting a request.
    ApprovalsDecide,
    /// Writes to the books: posting, reversing, bills, documents, accounts.
    Post,
    /// Moving money out (bill payments).
    MoneyOut,
    /// Closing or reopening a period or a fiscal year.
    Close,
}

impl Class {
    /// The class's name in the app and the API.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Class::Read => "read",
            Class::Settings => "settings",
            Class::Keys => "keys",
            Class::Audit => "audit",
            Class::ApprovalsRead => "approvals-read",
            Class::ApprovalsDecide => "approvals-decide",
            Class::Post => "post",
            Class::MoneyOut => "money-out",
            Class::Close => "close",
        }
    }
}

/// The class of a command, by its registry name. `None` for a name nobody
/// has classified: the matrix refuses it for every role.
#[must_use]
pub fn class_of(command: &str) -> Option<Class> {
    Some(match command {
        "company.get"
        | "company.profile"
        | "bank.accounts.list"
        | "bank.accounts.get"
        | "bank.transactions.list"
        | "bank.rules.list"
        | "reports.units_summary"
        | "tax.user_rates"
        | "units.list"
        | "accounts.get"
        | "accounts.list"
        | "entries.get"
        | "entries.list"
        | "reports.trial_balance"
        | "reports.profit_and_loss"
        | "reports.balance_sheet"
        | "reports.account_balance_by_dimension"
        | "reports.trends"
        | "documents.get"
        | "documents.list"
        | "documents.download_url"
        | "documents.anchors"
        | "bills.get"
        | "bills.list"
        | "fiscal_years.list"
        | "numbering.list"
        | "tax_settings.get"
        | "parties.list"
        | "connections.list"
        | "connections.runs"
        | "allocation_rules.list"
        | "reports.costs_by_unit"
        | "test.read" => Class::Read,
        "company.update"
        | "units.create"
        | "units.rename"
        | "units.archive"
        | "approvals.low_risk_limits"
        | "approvals.request_ttl_hours"
        | "tax_settings.update"
        | "approval_settings.get"
        | "approval_settings.update"
        | "numbering.set_next"
        | "parties.set_bank"
        | "parties.merge"
        | "connections.create"
        | "connections.update"
        | "connections.delete"
        | "connections.sync_now"
        | "allocation_rules.set" => Class::Settings,
        "keys.create" | "keys.list" | "keys.revoke" | "policy.put_key" | "policy.revoke_key"
        | "policy.explain" => Class::Keys,
        "audit.list" => Class::Audit,
        "approvals.get" | "approvals.list" => Class::ApprovalsRead,
        "approvals.decide" | "approvals.decide_bulk" => Class::ApprovalsDecide,
        "entries.post"
        | "entries.reverse"
        | "entries.transfer"
        | "journal.post_entry"
        | "bills.create"
        | "bills.update"
        | "bills.extract"
        | "bills.confirm"
        | "bills.discard"
        | "bills.post"
        | "documents.register"
        | "documents.set_anchors"
        | "documents.delete"
        | "accounts.create"
        | "accounts.update"
        | "accounts.archive"
        | "accounts.restore"
        | "bank.accounts.create"
        | "bank.accounts.update"
        | "bank.accounts.archive"
        | "bank.accounts.reconcile"
        | "bank.statements.import"
        | "bank.transactions.post"
        | "bank.transactions.link"
        | "bank.transactions.ignore"
        | "bank.rules.create"
        | "bank.rules.enable"
        | "parties.create"
        | "parties.update"
        | "parties.check_vat"
        | "test.write"
        | "test.post" => Class::Post,
        "bill.pay" => Class::MoneyOut,
        "periods.close"
        | "periods.reopen"
        | "fiscal_years.create"
        | "fiscal_years.close"
        | "test.human_only" => Class::Close,
        _ => return None,
    })
}

/// Whether `role` may run a command of `class`.
#[must_use]
pub fn allows(role: Role, class: Class) -> bool {
    match role {
        Role::Owner | Role::Admin => true,
        Role::Accountant => matches!(class, Class::Read | Class::Post),
        Role::Approver => matches!(
            class,
            Class::Read | Class::ApprovalsRead | Class::ApprovalsDecide
        ),
        Role::Viewer => matches!(class, Class::Read | Class::ApprovalsRead),
        Role::Auditor => matches!(class, Class::Read | Class::Audit | Class::ApprovalsRead),
    }
}

/// Whether `role` may run `command`.
///
/// # Errors
///
/// The reason the matrix refuses it: the command is unclassified, or the role
/// does not hold the command's class.
pub fn check(role: Role, command: &str) -> Result<(), String> {
    let Some(class) = class_of(command) else {
        return Err(format!("{command} is not in the permission matrix"));
    };
    if allows(role, class) {
        Ok(())
    } else {
        Err(format!(
            "the {role} role may not run {command}; ask an owner or admin"
        ))
    }
}

/// The commands an approval runs once it is approved. The approval is the
/// authorization for these, checked when the request is raised and decided
/// (docs/PLAN.md section 5), so the approver's role is not checked again for
/// them: an approver may decide `entries.post` without posting it directly.
pub const APPROVED_COMMANDS: &[&str] = &["entries.post", "entries.reverse"];

/// The matrix as the app shows it: each role with the classes it holds, in
/// the order of [`Role::ALL`].
#[must_use]
pub fn matrix() -> Vec<(Role, Vec<Class>)> {
    const CLASSES: [Class; 9] = [
        Class::Read,
        Class::Settings,
        Class::Keys,
        Class::Audit,
        Class::ApprovalsRead,
        Class::ApprovalsDecide,
        Class::Post,
        Class::MoneyOut,
        Class::Close,
    ];
    Role::ALL
        .into_iter()
        .map(|role| {
            let held = CLASSES
                .into_iter()
                .filter(|class| allows(role, *class))
                .collect();
            (role, held)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command the server registers, with the class the matrix gives it.
    /// Keep this table and the registry in step: a name missing here is a
    /// command with no role rule.
    const COMMANDS: &[(&str, Class)] = &[
        ("company.get", Class::Read),
        ("accounts.get", Class::Read),
        ("accounts.list", Class::Read),
        ("entries.get", Class::Read),
        ("entries.list", Class::Read),
        ("reports.trial_balance", Class::Read),
        ("reports.profit_and_loss", Class::Read),
        ("reports.balance_sheet", Class::Read),
        ("reports.account_balance_by_dimension", Class::Read),
        ("reports.trends", Class::Read),
        ("documents.get", Class::Read),
        ("documents.list", Class::Read),
        ("documents.download_url", Class::Read),
        ("documents.anchors", Class::Read),
        ("bills.get", Class::Read),
        ("bills.list", Class::Read),
        ("fiscal_years.list", Class::Read),
        ("numbering.list", Class::Read),
        ("tax_settings.get", Class::Read),
        ("connections.list", Class::Read),
        ("connections.runs", Class::Read),
        ("allocation_rules.list", Class::Read),
        ("reports.costs_by_unit", Class::Read),
        ("connections.create", Class::Settings),
        ("connections.update", Class::Settings),
        ("connections.delete", Class::Settings),
        ("connections.sync_now", Class::Settings),
        ("allocation_rules.set", Class::Settings),
        ("company.update", Class::Settings),
        ("tax_settings.update", Class::Settings),
        ("approval_settings.get", Class::Settings),
        ("approval_settings.update", Class::Settings),
        ("numbering.set_next", Class::Settings),
        ("keys.create", Class::Keys),
        ("keys.list", Class::Keys),
        ("keys.revoke", Class::Keys),
        ("policy.put_key", Class::Keys),
        ("policy.revoke_key", Class::Keys),
        ("policy.explain", Class::Keys),
        ("audit.list", Class::Audit),
        ("approvals.get", Class::ApprovalsRead),
        ("approvals.list", Class::ApprovalsRead),
        ("approvals.decide", Class::ApprovalsDecide),
        ("entries.post", Class::Post),
        ("entries.reverse", Class::Post),
        ("journal.post_entry", Class::Post),
        ("bills.create", Class::Post),
        ("bills.update", Class::Post),
        ("bills.extract", Class::Post),
        ("bills.confirm", Class::Post),
        ("bills.discard", Class::Post),
        ("bills.post", Class::Post),
        ("documents.register", Class::Post),
        ("documents.set_anchors", Class::Post),
        ("documents.delete", Class::Post),
        ("accounts.create", Class::Post),
        ("accounts.update", Class::Post),
        ("accounts.archive", Class::Post),
        ("accounts.restore", Class::Post),
        ("bill.pay", Class::MoneyOut),
        ("periods.close", Class::Close),
        ("periods.reopen", Class::Close),
        ("fiscal_years.create", Class::Close),
        ("fiscal_years.close", Class::Close),
        ("parties.list", Class::Read),
        ("parties.create", Class::Post),
        ("parties.update", Class::Post),
        ("parties.check_vat", Class::Post),
        ("parties.set_bank", Class::Settings),
        ("parties.merge", Class::Settings),
    ];

    /// The table, role by role: a role runs a command exactly when the matrix
    /// gives the role the command's class. Each case is one cell.
    #[test]
    fn every_command_is_classified_and_every_role_is_checked_per_class() {
        for (command, class) in COMMANDS {
            assert_eq!(class_of(command), Some(*class), "{command}");
            for role in Role::ALL {
                let expected = allows(role, *class);
                assert_eq!(
                    check(role, command).is_ok(),
                    expected,
                    "{role} running {command} ({class:?})"
                );
            }
        }
    }

    /// The role table from docs/permissions.md, spelled out cell by cell so a
    /// change to the matrix has to change this test too.
    #[test]
    fn the_matrix_matches_the_documented_roles() {
        let cases: &[(Role, &str, bool)] = &[
            (Role::Owner, "keys.create", true),
            (Role::Owner, "bill.pay", true),
            (Role::Owner, "periods.close", true),
            (Role::Admin, "company.update", true),
            (Role::Admin, "approvals.decide", true),
            (Role::Admin, "bill.pay", true),
            (Role::Accountant, "entries.post", true),
            (Role::Accountant, "entries.reverse", true),
            (Role::Accountant, "bills.create", true),
            (Role::Accountant, "accounts.create", true),
            (Role::Accountant, "reports.trial_balance", true),
            (Role::Accountant, "company.update", false),
            (Role::Accountant, "keys.create", false),
            (Role::Accountant, "audit.list", false),
            (Role::Accountant, "approvals.decide", false),
            (Role::Accountant, "bill.pay", false),
            (Role::Accountant, "periods.close", false),
            (Role::Approver, "approvals.decide", true),
            (Role::Approver, "approvals.list", true),
            (Role::Approver, "entries.list", true),
            (Role::Approver, "entries.post", false),
            (Role::Approver, "keys.list", false),
            (Role::Approver, "audit.list", false),
            (Role::Viewer, "entries.list", true),
            (Role::Viewer, "reports.balance_sheet", true),
            (Role::Viewer, "entries.post", false),
            (Role::Viewer, "approvals.decide", false),
            (Role::Viewer, "audit.list", false),
            (Role::Auditor, "audit.list", true),
            (Role::Auditor, "entries.list", true),
            (Role::Auditor, "reports.trial_balance", true),
            (Role::Auditor, "entries.post", false),
            (Role::Auditor, "approvals.decide", false),
            (Role::Auditor, "keys.list", false),
            (Role::Owner, "parties.set_bank", true),
            (Role::Admin, "parties.merge", true),
            (Role::Accountant, "parties.create", true),
            (Role::Accountant, "parties.check_vat", true),
            (Role::Accountant, "parties.set_bank", false),
            (Role::Accountant, "parties.merge", false),
            (Role::Approver, "parties.list", true),
            (Role::Approver, "parties.update", false),
            (Role::Viewer, "parties.merge", false),
        ];
        for (role, command, allowed) in cases {
            assert_eq!(check(*role, command).is_ok(), *allowed, "{role} {command}");
        }
    }

    #[test]
    fn an_unclassified_command_is_refused_for_every_role() {
        for role in Role::ALL {
            let reason = check(role, "not.a.command").expect_err("refused");
            assert!(reason.contains("permission matrix"), "{reason}");
        }
    }

    #[test]
    fn the_matrix_view_lists_each_role_in_order() {
        let view = matrix();
        let roles: Vec<Role> = view.iter().map(|(role, _)| *role).collect();
        assert_eq!(roles, Role::ALL.to_vec());
        let auditor = view
            .iter()
            .find(|(role, _)| *role == Role::Auditor)
            .map(|(_, classes)| classes.clone())
            .expect("auditor listed");
        assert!(auditor.contains(&Class::Audit));
        assert!(!auditor.contains(&Class::Post));
    }
}

#[cfg(test)]
mod connections_roles {
    use super::*;

    /// Connections and their credentials are an owner and admin matter: an
    /// accountant who can post may read the costs but not connect a vendor.
    #[test]
    fn only_owners_and_admins_connect_sources_and_read_is_open_to_the_books() {
        for name in [
            "connections.create",
            "connections.update",
            "connections.delete",
            "connections.sync_now",
            "allocation_rules.set",
        ] {
            assert!(check(Role::Owner, name).is_ok(), "owner may {name}");
            assert!(check(Role::Admin, name).is_ok(), "admin may {name}");
            assert!(
                check(Role::Accountant, name).is_err(),
                "accountant may not {name}"
            );
            assert!(check(Role::Viewer, name).is_err(), "viewer may not {name}");
        }
        for name in [
            "connections.list",
            "connections.runs",
            "allocation_rules.list",
            "reports.costs_by_unit",
        ] {
            assert!(check(Role::Viewer, name).is_ok(), "viewer may read {name}");
            assert!(
                check(Role::Accountant, name).is_ok(),
                "accountant may read {name}"
            );
        }
    }

    #[test]
    fn an_unclassified_connections_name_is_refused_for_everyone() {
        assert!(class_of("connections.anything_new").is_none());
    }
}
