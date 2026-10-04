use async_trait::async_trait;
use panel_health::{CheckOutcome, ComponentType, HealthCheck};
use sqlx::SqlitePool;

/// Passes while a pooled connection can run a trivial query.
pub struct SqliteHealthCheck {
    pool: SqlitePool,
}

impl SqliteHealthCheck {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HealthCheck for SqliteHealthCheck {
    fn component(&self) -> &str {
        "sqlite"
    }

    fn component_type(&self) -> ComponentType {
        ComponentType::Datastore
    }

    async fn check(&self) -> CheckOutcome {
        if self.pool.is_closed() {
            return CheckOutcome::fail("pool closed");
        }
        match sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.pool)
            .await
        {
            Ok(_) => CheckOutcome::pass(),
            Err(_) => CheckOutcome::fail("query failed"),
        }
    }
}
