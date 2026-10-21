//! E-invoice transport: send and receive e-invoices.

use super::scope::CompanyId;
use async_trait::async_trait;
use ledgers_commands::CommandError;

/// E-invoice: format, filename, content.
#[derive(Clone, Debug)]
pub struct EInvoice {
    pub format: String,
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// Outcome of sending an e-invoice.
#[derive(Clone, Debug)]
pub enum Delivery {
    Exported(EInvoice),
    Sent { reference: String },
}

/// E-invoice transport: send and receive e-invoices.
#[async_trait(?Send)]
pub trait EInvoiceTransport: Send + Sync {
    /// Sends an e-invoice.
    async fn send(&self, company: &CompanyId, invoice: &EInvoice)
    -> Result<Delivery, CommandError>;

    /// Receives e-invoices.
    async fn receive(&self, company: &CompanyId) -> Result<Vec<EInvoice>, CommandError>;
}

/// Open default: file export and import via intake.
#[derive(Clone, Copy, Debug)]
pub struct FileExchange;

#[async_trait(?Send)]
impl EInvoiceTransport for FileExchange {
    async fn send(
        &self,
        _company: &CompanyId,
        invoice: &EInvoice,
    ) -> Result<Delivery, CommandError> {
        Ok(Delivery::Exported(invoice.clone()))
    }

    async fn receive(&self, _company: &CompanyId) -> Result<Vec<EInvoice>, CommandError> {
        Ok(Vec::new())
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{CompanyId, Delivery, EInvoice, EInvoiceTransport};

    /// Contract test for `EInvoiceTransport` implementations.
    ///
    /// # Panics
    /// Panics if a `Delivery::Sent` result has an empty reference.
    pub async fn e_invoice_transport(transport: &dyn EInvoiceTransport, company: &CompanyId) {
        let invoice = EInvoice {
            format: "xrechnung".to_string(),
            file_name: "test.xml".to_string(),
            bytes: b"test".to_vec(),
        };

        if let Ok(Delivery::Sent { reference }) = transport.send(company, &invoice).await {
            assert!(!reference.is_empty());
        }

        let _ = transport.receive(company).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contract_tests_pass() {
        let transport = FileExchange;
        let company = CompanyId::new("test".to_string());
        contract_tests::e_invoice_transport(&transport, &company).await;
    }
}
