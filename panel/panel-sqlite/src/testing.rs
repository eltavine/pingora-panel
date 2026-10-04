//! Disposable databases for tests: a fresh file in a temporary directory
//! that is removed with it.

use crate::{SchemaMigration, ServiceDatabase, ServiceDatabaseConfig};
use std::path::Path;
use tempfile::TempDir;

/// A migrated database in its own temporary directory.
pub struct TestDatabase {
    database: ServiceDatabase,
    directory: TempDir,
}

impl TestDatabase {
    /// A database with the platform tables and `migrations` applied.
    pub async fn migrated(migrations: &[SchemaMigration]) -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database =
            ServiceDatabase::open(ServiceDatabaseConfig::at(directory.path().join("test.db")))
                .expect("the test database opens");
        database
            .migrate(migrations)
            .await
            .expect("the migrations apply");
        Self {
            database,
            directory,
        }
    }

    pub fn database(&self) -> &ServiceDatabase {
        &self.database
    }

    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
}
