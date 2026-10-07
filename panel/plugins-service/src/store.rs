//! What the plugins module keeps: each plugin's grants, settings, limits
//! and versions, the trusted publisher keys and the sealed secrets.

use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_plugin_api::{PluginLimits, SecretView, TrustedKeyView};
use panel_secrets::Sealed;
use panel_sqlite::{storage_error, ServiceDatabase};
use serde_json::Value;
use sqlx::{sqlite::SqliteRow, Row, SqliteConnection};

/// What administrators decided about a plugin.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginRecord {
    pub name: String,
    pub enabled: bool,
    pub active_version: Option<String>,
    pub previous_version: Option<String>,
    pub grants: Vec<String>,
    pub settings: Value,
    pub limits: PluginLimits,
    /// Zero until first saved.
    pub version: u64,
    pub updated_at: Option<DateTime<Utc>>,
}

impl PluginRecord {
    /// A plugin nothing was decided about: disabled, granted nothing.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            enabled: false,
            active_version: None,
            previous_version: None,
            grants: Vec::new(),
            settings: Value::Object(serde_json::Map::new()),
            limits: PluginLimits::default(),
            version: 0,
            updated_at: None,
        }
    }

    pub fn etag(&self) -> String {
        format!("\"{}\"", self.version)
    }
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("stored values serialize")
}

fn corrupt(what: &str) -> PanelError {
    PanelError::corrupt_state(format!("a stored plugin has an invalid {what}"))
}

fn plugin(row: &SqliteRow) -> Result<PluginRecord> {
    let text = |column: &str| row.try_get::<String, _>(column).map_err(storage_error);
    let version: i64 = row.try_get("version").map_err(storage_error)?;
    Ok(PluginRecord {
        name: text("name")?,
        enabled: row.try_get::<i64, _>("enabled").map_err(storage_error)? == 1,
        active_version: row.try_get("active_version").map_err(storage_error)?,
        previous_version: row.try_get("previous_version").map_err(storage_error)?,
        grants: serde_json::from_str(&text("grants")?).map_err(|_| corrupt("grant"))?,
        settings: serde_json::from_str(&text("settings")?).map_err(|_| corrupt("setting"))?,
        limits: serde_json::from_str(&text("limits")?).map_err(|_| corrupt("limit"))?,
        version: u64::try_from(version).map_err(|_| corrupt("version"))?,
        updated_at: Some(row.try_get("updated_at").map_err(storage_error)?),
    })
}

fn key(row: &SqliteRow) -> Result<TrustedKeyView> {
    let mut key = TrustedKeyView::default();
    key.id = row.try_get("id").map_err(storage_error)?;
    key.key_id = row.try_get("key_id").map_err(storage_error)?;
    key.public_key = row.try_get("public_key").map_err(storage_error)?;
    key.comment = row.try_get("comment").map_err(storage_error)?;
    key.created_at = row.try_get("created_at").map_err(storage_error)?;
    Ok(key)
}

macro_rules! plugin_columns {
    () => {
        "name, enabled, active_version, previous_version, grants, settings, limits, version, \
         updated_at"
    };
}

#[derive(Clone)]
pub struct Store {
    database: ServiceDatabase,
}

impl Store {
    pub fn new(database: &ServiceDatabase) -> Self {
        Self {
            database: database.clone(),
        }
    }

    pub fn database(&self) -> &ServiceDatabase {
        &self.database
    }

    pub async fn plugins(&self) -> Result<Vec<PluginRecord>> {
        sqlx::query(concat!(
            "SELECT ",
            plugin_columns!(),
            " FROM plugins ORDER BY name"
        ))
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?
        .iter()
        .map(plugin)
        .collect()
    }

    pub async fn plugin(&self, name: &str) -> Result<Option<PluginRecord>> {
        sqlx::query(concat!(
            "SELECT ",
            plugin_columns!(),
            " FROM plugins WHERE name = ?1"
        ))
        .bind(name)
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        .map(|row| plugin(&row))
        .transpose()
    }

    /// Writes `record`, which must be one version past what is stored.
    pub async fn save(connection: &mut SqliteConnection, record: &PluginRecord) -> Result<()> {
        let version = i64::try_from(record.version).map_err(|_| corrupt("version"))?;
        let written = sqlx::query(
            "INSERT INTO plugins (name, enabled, active_version, previous_version, grants, \
             settings, limits, version, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT (name) DO UPDATE SET enabled = excluded.enabled, \
             active_version = excluded.active_version, \
             previous_version = excluded.previous_version, grants = excluded.grants, \
             settings = excluded.settings, limits = excluded.limits, \
             version = excluded.version, updated_at = excluded.updated_at \
             WHERE plugins.version = excluded.version - 1",
        )
        .bind(&record.name)
        .bind(i64::from(record.enabled))
        .bind(&record.active_version)
        .bind(&record.previous_version)
        .bind(json(&record.grants))
        .bind(json(&record.settings))
        .bind(json(&record.limits))
        .bind(version)
        .bind(record.updated_at.unwrap_or_else(Utc::now))
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?
        .rows_affected();
        if written == 0 {
            return Err(PanelError::conflict(format!(
                "plugin {} changed while this change was made",
                record.name
            )));
        }
        Ok(())
    }

    pub async fn keys(&self) -> Result<Vec<TrustedKeyView>> {
        sqlx::query(
            "SELECT id, key_id, public_key, comment, created_at FROM trusted_keys ORDER BY id",
        )
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?
        .iter()
        .map(key)
        .collect()
    }

    pub async fn put_key(connection: &mut SqliteConnection, key: &TrustedKeyView) -> Result<()> {
        let written = sqlx::query(
            "INSERT INTO trusted_keys (id, key_id, public_key, comment, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
        )
        .bind(&key.id)
        .bind(&key.key_id)
        .bind(&key.public_key)
        .bind(&key.comment)
        .bind(key.created_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?
        .rows_affected();
        if written == 0 {
            return Err(PanelError::conflict(format!(
                "key {} or a key with ID {} is already trusted",
                key.id, key.key_id
            )));
        }
        Ok(())
    }

    pub async fn delete_key(connection: &mut SqliteConnection, id: &str) -> Result<()> {
        let deleted = sqlx::query("DELETE FROM trusted_keys WHERE id = ?1")
            .bind(id)
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?
            .rows_affected();
        if deleted == 0 {
            return Err(PanelError::not_found(format!(
                "there is no trusted key {id}"
            )));
        }
        Ok(())
    }

    pub async fn secrets(&self) -> Result<Vec<SecretView>> {
        sqlx::query("SELECT name, updated_at FROM secrets ORDER BY name")
            .fetch_all(self.database.pool())
            .await
            .map_err(storage_error)?
            .iter()
            .map(|row| {
                let mut secret = SecretView::default();
                secret.name = row.try_get("name").map_err(storage_error)?;
                secret.updated_at = row.try_get("updated_at").map_err(storage_error)?;
                Ok(secret)
            })
            .collect()
    }

    pub async fn sealed(&self, name: &str) -> Result<Option<Sealed>> {
        let sealed: Option<String> =
            sqlx::query_scalar("SELECT sealed FROM secrets WHERE name = ?1")
                .bind(name)
                .fetch_optional(self.database.pool())
                .await
                .map_err(storage_error)?;
        Ok(sealed.map(Sealed::new))
    }

    pub async fn put_secret(
        connection: &mut SqliteConnection,
        name: &str,
        sealed: &Sealed,
        now: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO secrets (name, sealed, updated_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, \
             updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(sealed.as_str())
        .bind(now)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    pub async fn delete_secret(connection: &mut SqliteConnection, name: &str) -> Result<()> {
        let deleted = sqlx::query("DELETE FROM secrets WHERE name = ?1")
            .bind(name)
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?
            .rows_affected();
        if deleted == 0 {
            return Err(PanelError::not_found(format!("there is no secret {name}")));
        }
        Ok(())
    }
}
