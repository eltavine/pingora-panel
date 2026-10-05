//! Backups (ADR 0035): archives of the control plane's databases and the
//! sites' directory, taken in the background, downloaded, restored and
//! removed through a port of their own.

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, fmt, sync::Arc, time::SystemTime};

/// Where a configuration backup keeps the draft as a configuration bundle.
pub const DRAFT_BUNDLE: &str = "configuration/draft.json";
/// Where a configuration backup keeps the active revision as a bundle.
pub const ACTIVE_BUNDLE: &str = "configuration/active.json";

/// What a backup holds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum BackupContent {
    /// The configuration's database, with the draft and the active
    /// revision as configuration bundles.
    Configuration,
    /// Certificates with their keys sealed, ACME accounts and DNS
    /// providers.
    Certificates,
    /// Every module's database.
    Databases,
    /// The sites' directory, or one directory below it.
    Sites,
}

impl BackupContent {
    pub const ALL: [Self; 4] = [
        Self::Configuration,
        Self::Certificates,
        Self::Databases,
        Self::Sites,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Certificates => "certificates",
            Self::Databases => "databases",
            Self::Sites => "sites",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|content| content.as_str() == value)
    }

    /// Whether the configuration's database is in a backup holding this.
    pub fn holds_configuration(self) -> bool {
        matches!(self, Self::Configuration | Self::Databases)
    }
}

/// How far taking a backup got.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BackupState {
    Pending,
    Running,
    Completed,
    Failed,
}

impl BackupState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// A backup and how far taking it got.
#[derive(Clone, Debug)]
pub struct Backup {
    pub id: String,
    pub contents: Vec<BackupContent>,
    /// The directory below the sites' directory it holds; empty for all.
    pub site_path: String,
    pub state: BackupState,
    pub requested_by: String,
    pub requested_at: SystemTime,
    pub finished_at: Option<SystemTime>,
    /// The archive's size and SHA-256 in lowercase hexadecimal, once taken.
    pub size_bytes: u64,
    pub sha256: String,
    pub files: u64,
    pub failure: Option<PanelError>,
    pub product_version: String,
}

/// What to back up.
#[derive(Clone, Default)]
pub struct BackupRequest {
    pub contents: Vec<BackupContent>,
    pub site_path: String,
    /// Members written into the archive as given, below `configuration/`.
    pub attachments: BTreeMap<String, Vec<u8>>,
}

impl fmt::Debug for BackupRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BackupRequest")
            .field("contents", &self.contents)
            .field("site_path", &self.site_path)
            .field("attachments", &self.attachments.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// A backup's archive: the backup, then its bytes in order.
pub struct BackupDownload {
    pub backup: Backup,
    pub chunks: BoxStream<'static, Result<Vec<u8>>>,
}

/// What restoring a directory of the sites wrote.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SitesRestored {
    pub files: u64,
    pub bytes: u64,
}

#[async_trait]
pub trait BackupsPort: Send + Sync {
    /// Every backup, newest first.
    async fn list(&self, scope: RequestScope) -> Result<Vec<Backup>>;
    async fn get(&self, scope: RequestScope, id: &str) -> Result<Backup>;
    /// Lists a backup as pending and takes it in the background.
    async fn create(&self, context: CommandContext, request: BackupRequest) -> Result<Backup>;
    /// Removes a backup and its archive; one being taken is refused.
    async fn delete(&self, context: CommandContext, id: &str) -> Result<()>;
    async fn download(&self, scope: RequestScope, id: &str) -> Result<BackupDownload>;
    /// One member of at most 16 MiB of a taken backup's archive.
    async fn member(&self, scope: RequestScope, id: &str, path: &str) -> Result<Vec<u8>>;
    /// Replaces a directory below the sites' directory with the backup's
    /// copy.
    async fn restore_sites(
        &self,
        context: CommandContext,
        id: &str,
        site_path: &str,
    ) -> Result<SitesRestored>;
}

/// Backups where nothing takes them.
pub struct NoBackups;

impl NoBackups {
    fn refusal() -> PanelError {
        PanelError::unsupported_capability("backups are not taken here")
    }
}

#[async_trait]
impl BackupsPort for NoBackups {
    async fn list(&self, _: RequestScope) -> Result<Vec<Backup>> {
        Err(Self::refusal())
    }

    async fn get(&self, _: RequestScope, _: &str) -> Result<Backup> {
        Err(Self::refusal())
    }

    async fn create(&self, _: CommandContext, _: BackupRequest) -> Result<Backup> {
        Err(Self::refusal())
    }

    async fn delete(&self, _: CommandContext, _: &str) -> Result<()> {
        Err(Self::refusal())
    }

    async fn download(&self, _: RequestScope, _: &str) -> Result<BackupDownload> {
        Err(Self::refusal())
    }

    async fn member(&self, _: RequestScope, _: &str, _: &str) -> Result<Vec<u8>> {
        Err(Self::refusal())
    }

    async fn restore_sites(&self, _: CommandContext, _: &str, _: &str) -> Result<SitesRestored> {
        Err(Self::refusal())
    }
}

/// A change of the backups, as the audit trail records it.
#[derive(Clone, Copy, Debug)]
pub enum BackupChange<'a> {
    Requested(std::result::Result<&'a Backup, &'a PanelError>),
    Deleted(std::result::Result<(), &'a PanelError>),
    SitesRestored {
        site_path: &'a str,
        result: std::result::Result<&'a SitesRestored, &'a PanelError>,
    },
    /// The configuration was restored into the draft, at this version.
    ConfigurationRestored(std::result::Result<u64, &'a PanelError>),
}

/// A backups port that records each change, refused or not.
pub struct RecordedBackups {
    inner: Arc<dyn BackupsPort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedBackups {
    pub fn new(inner: Arc<dyn BackupsPort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl BackupsPort for RecordedBackups {
    async fn list(&self, scope: RequestScope) -> Result<Vec<Backup>> {
        self.inner.list(scope).await
    }

    async fn get(&self, scope: RequestScope, id: &str) -> Result<Backup> {
        self.inner.get(scope, id).await
    }

    async fn create(&self, context: CommandContext, request: BackupRequest) -> Result<Backup> {
        let result = self.inner.create(context.clone(), request).await;
        let operation = Operation::Backup {
            id: result.as_ref().map_or("", |backup| backup.id.as_str()),
            change: BackupChange::Requested(result.as_ref()),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn delete(&self, context: CommandContext, id: &str) -> Result<()> {
        let result = self.inner.delete(context.clone(), id).await;
        let operation = Operation::Backup {
            id,
            change: BackupChange::Deleted(result.as_ref().copied()),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn download(&self, scope: RequestScope, id: &str) -> Result<BackupDownload> {
        self.inner.download(scope, id).await
    }

    async fn member(&self, scope: RequestScope, id: &str, path: &str) -> Result<Vec<u8>> {
        self.inner.member(scope, id, path).await
    }

    async fn restore_sites(
        &self,
        context: CommandContext,
        id: &str,
        site_path: &str,
    ) -> Result<SitesRestored> {
        let result = self
            .inner
            .restore_sites(context.clone(), id, site_path)
            .await;
        let operation = Operation::Backup {
            id,
            change: BackupChange::SitesRestored {
                site_path,
                result: result.as_ref(),
            },
        };
        self.log.record(&context, operation).await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what each operation was and whether it succeeded.
    #[derive(Default)]
    struct Log(Mutex<Vec<(String, bool)>>);

    #[async_trait]
    impl OperationLog for Log {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            let Operation::Backup { id, change } = operation else {
                panic!("only backups are recorded here");
            };
            let (what, done) = match change {
                BackupChange::Requested(result) => ("requested", result.is_ok()),
                BackupChange::Deleted(result) => ("deleted", result.is_ok()),
                BackupChange::SitesRestored { result, .. } => ("restored", result.is_ok()),
                BackupChange::ConfigurationRestored(result) => ("configuration", result.is_ok()),
            };
            self.0.lock().unwrap().push((format!("{what} {id}"), done));
        }
    }

    fn context() -> CommandContext {
        CommandContext::new(
            panel_context::RequestId::new("request-1").unwrap(),
            panel_context::RequestId::new("request-1").unwrap(),
            "alice",
            panel_context::RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            panel_context::IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn every_change_is_recorded_refused_or_not() {
        let log = Arc::new(Log::default());
        let backups = RecordedBackups::new(Arc::new(NoBackups), log.clone());
        assert!(backups
            .create(context(), BackupRequest::default())
            .await
            .is_err());
        assert!(backups.delete(context(), "b-1").await.is_err());
        assert!(backups
            .restore_sites(context(), "b-1", "shop")
            .await
            .is_err());
        assert_eq!(
            *log.0.lock().unwrap(),
            [
                ("requested ".to_owned(), false),
                ("deleted b-1".to_owned(), false),
                ("restored b-1".to_owned(), false),
            ]
        );
    }

    #[test]
    fn contents_round_trip_through_their_names() {
        for content in BackupContent::ALL {
            assert_eq!(BackupContent::parse(content.as_str()), Some(content));
        }
        assert!(BackupContent::Databases.holds_configuration());
        assert!(!BackupContent::Sites.holds_configuration());
    }
}
