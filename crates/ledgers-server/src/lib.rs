//! Placeholder for the Ledgers server — the harness composition of the Worker
//! and native runtimes lands in later issues; see docs/PLAN.md §6 and
//! docs/adr/0001 for the pinned harness.

/// Placeholder marker type for the future Ledgers server composition.
#[derive(Debug)]
pub struct Server;

#[cfg(test)]
mod tests {
    use super::Server;

    #[test]
    fn placeholder_type_is_unit_struct() {
        let server = Server;
        assert_eq!(format!("{server:?}"), "Server");
    }
}
