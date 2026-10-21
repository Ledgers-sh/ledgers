//! Document intake: where documents come from.

use super::scope::CompanyId;
use async_trait::async_trait;
use ledgers_commands::CommandError;

/// Intake source: document origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntakeSource {
    Upload,
    Folder(String),
}

/// Incoming document: source, filename, media type, content.
#[derive(Clone, Debug)]
pub struct IncomingDocument {
    pub source: IntakeSource,
    pub file_name: String,
    pub media_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// Document intake: define sources and poll for new documents.
#[async_trait(?Send)]
pub trait DocumentIntake: Send + Sync {
    /// The sources this intake uses.
    fn sources(&self) -> Vec<IntakeSource>;

    /// Polls for new documents.
    async fn poll(&self, company: &CompanyId) -> Result<Vec<IncomingDocument>, CommandError>;
}

/// Open default: HTTP upload and optional folder watch.
#[derive(Clone, Debug)]
pub struct UploadAndFolder {
    pub folder: Option<String>,
}

impl UploadAndFolder {
    /// Creates an intake with HTTP upload only.
    #[must_use]
    pub fn upload_only() -> Self {
        UploadAndFolder { folder: None }
    }
}

#[async_trait(?Send)]
impl DocumentIntake for UploadAndFolder {
    fn sources(&self) -> Vec<IntakeSource> {
        let mut sources = vec![IntakeSource::Upload];
        if let Some(ref folder) = self.folder {
            sources.push(IntakeSource::Folder(folder.clone()));
        }
        sources
    }

    async fn poll(&self, _company: &CompanyId) -> Result<Vec<IncomingDocument>, CommandError> {
        Ok(Vec::new())
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{CompanyId, DocumentIntake};

    /// Contract test for `DocumentIntake` implementations.
    ///
    /// # Panics
    /// Panics if sources list is empty.
    pub async fn document_intake(intake: &dyn DocumentIntake, company: &CompanyId) {
        assert!(!intake.sources().is_empty());
        let _ = intake.poll(company).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_only_sources() {
        let intake = UploadAndFolder::upload_only();
        assert!(intake.sources().contains(&IntakeSource::Upload));
    }

    #[tokio::test]
    async fn contract_tests_pass() {
        let intake = UploadAndFolder::upload_only();
        let company = CompanyId::new("test".to_string());
        contract_tests::document_intake(&intake, &company).await;
    }
}
