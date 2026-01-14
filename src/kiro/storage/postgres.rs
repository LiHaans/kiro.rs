//! PostgreSQL 凭据存储实现
//!
//! 需要启用 `postgres` feature

use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use sqlx::{postgres::PgPoolOptions, PgPool, Row};

use crate::kiro::model::credentials::KiroCredentials;

use super::traits::CredentialStorage;

/// PostgreSQL 凭据存储
pub struct PostgresCredentialStorage {
    /// 数据库连接池
    pool: PgPool,
    /// 凭据表名
    table_name: String,
    /// 上次同步时间戳（Unix 秒）
    last_sync: AtomicI64,
}

impl PostgresCredentialStorage {
    /// 创建 PostgreSQL 存储实例
    ///
    /// # Arguments
    /// * `database_url` - 数据库连接 URL，格式: postgres://user:password@host:port/database
    /// * `table_name` - 凭据表名
    /// * `max_connections` - 连接池最大连接数
    pub async fn new(
        database_url: &str,
        table_name: &str,
        max_connections: u32,
    ) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(database_url)
            .await?;

        tracing::info!(
            "PostgreSQL 连接池已创建，表名: {}，最大连接数: {}",
            table_name,
            max_connections
        );

        Ok(Self {
            pool,
            table_name: table_name.to_string(),
            last_sync: AtomicI64::new(0),
        })
    }

    /// 更新最后同步时间
    pub fn update_last_sync(&self) {
        let now = chrono::Utc::now().timestamp();
        self.last_sync.store(now, Ordering::Relaxed);
    }

    /// 获取最后同步时间
    pub fn last_sync_timestamp(&self) -> i64 {
        self.last_sync.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl CredentialStorage for PostgresCredentialStorage {
    async fn load_all(&self) -> anyhow::Result<Vec<KiroCredentials>> {
        let query = format!(
            r#"
            SELECT
                id, access_token, refresh_token, profile_arn, expires_at,
                auth_method, client_id, client_secret, priority, region, machine_id
            FROM {}
            WHERE deleted_at IS NULL
            ORDER BY priority ASC, id ASC
            "#,
            self.table_name
        );

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;

        let credentials: Vec<KiroCredentials> = rows
            .into_iter()
            .map(|row| {
                let expires_at: Option<chrono::DateTime<chrono::Utc>> = row.get("expires_at");
                KiroCredentials {
                    id: row.get::<Option<i64>, _>("id").map(|id| id as u64),
                    access_token: row.get("access_token"),
                    refresh_token: row.get("refresh_token"),
                    profile_arn: row.get("profile_arn"),
                    expires_at: expires_at.map(|dt| dt.to_rfc3339()),
                    auth_method: row.get("auth_method"),
                    client_id: row.get("client_id"),
                    client_secret: row.get("client_secret"),
                    priority: row.get::<Option<i32>, _>("priority").unwrap_or(0) as u32,
                    region: row.get("region"),
                    machine_id: row.get("machine_id"),
                }
            })
            .collect();

        self.update_last_sync();
        tracing::debug!("从 PostgreSQL 加载了 {} 个凭据", credentials.len());

        Ok(credentials)
    }

    async fn save(&self, credential: &KiroCredentials) -> anyhow::Result<()> {
        let expires_at = credential
            .expires_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        let query = format!(
            r#"
            INSERT INTO {} (id, access_token, refresh_token, profile_arn, expires_at,
                           auth_method, client_id, client_secret, priority, region, machine_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (id) DO UPDATE SET
                access_token = EXCLUDED.access_token,
                refresh_token = EXCLUDED.refresh_token,
                profile_arn = EXCLUDED.profile_arn,
                expires_at = EXCLUDED.expires_at,
                auth_method = EXCLUDED.auth_method,
                client_id = EXCLUDED.client_id,
                client_secret = EXCLUDED.client_secret,
                priority = EXCLUDED.priority,
                region = EXCLUDED.region,
                machine_id = EXCLUDED.machine_id,
                updated_at = NOW()
            "#,
            self.table_name
        );

        sqlx::query(&query)
            .bind(credential.id.map(|id| id as i64))
            .bind(&credential.access_token)
            .bind(&credential.refresh_token)
            .bind(&credential.profile_arn)
            .bind(expires_at)
            .bind(&credential.auth_method)
            .bind(&credential.client_id)
            .bind(&credential.client_secret)
            .bind(credential.priority as i32)
            .bind(&credential.region)
            .bind(&credential.machine_id)
            .execute(&self.pool)
            .await?;

        tracing::debug!("已保存凭据到 PostgreSQL: id={:?}", credential.id);
        Ok(())
    }

    async fn save_all(&self, credentials: &[KiroCredentials]) -> anyhow::Result<()> {
        // 使用事务批量保存
        let mut tx = self.pool.begin().await?;

        for credential in credentials {
            let expires_at = credential
                .expires_at
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            let query = format!(
                r#"
                INSERT INTO {} (id, access_token, refresh_token, profile_arn, expires_at,
                               auth_method, client_id, client_secret, priority, region, machine_id)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (id) DO UPDATE SET
                    access_token = EXCLUDED.access_token,
                    refresh_token = EXCLUDED.refresh_token,
                    profile_arn = EXCLUDED.profile_arn,
                    expires_at = EXCLUDED.expires_at,
                    auth_method = EXCLUDED.auth_method,
                    client_id = EXCLUDED.client_id,
                    client_secret = EXCLUDED.client_secret,
                    priority = EXCLUDED.priority,
                    region = EXCLUDED.region,
                    machine_id = EXCLUDED.machine_id,
                    updated_at = NOW()
                "#,
                self.table_name
            );

            sqlx::query(&query)
                .bind(credential.id.map(|id| id as i64))
                .bind(&credential.access_token)
                .bind(&credential.refresh_token)
                .bind(&credential.profile_arn)
                .bind(expires_at)
                .bind(&credential.auth_method)
                .bind(&credential.client_id)
                .bind(&credential.client_secret)
                .bind(credential.priority as i32)
                .bind(&credential.region)
                .bind(&credential.machine_id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        tracing::debug!("已批量保存 {} 个凭据到 PostgreSQL", credentials.len());
        Ok(())
    }

    async fn delete(&self, id: u64) -> anyhow::Result<()> {
        // 软删除
        let query = format!(
            "UPDATE {} SET deleted_at = NOW(), updated_at = NOW() WHERE id = $1",
            self.table_name
        );

        sqlx::query(&query)
            .bind(id as i64)
            .execute(&self.pool)
            .await?;

        tracing::debug!("已从 PostgreSQL 删除凭据: id={}", id);
        Ok(())
    }

    fn storage_type(&self) -> &'static str {
        "postgresql"
    }

    async fn has_changes_since(&self, since_timestamp: i64) -> anyhow::Result<bool> {
        let query = format!(
            "SELECT COUNT(*) as count FROM {} WHERE updated_at > to_timestamp($1) OR deleted_at > to_timestamp($1)",
            self.table_name
        );

        let row = sqlx::query(&query)
            .bind(since_timestamp as f64)
            .fetch_one(&self.pool)
            .await?;

        let count: i64 = row.get("count");
        Ok(count > 0)
    }
}

/// 创建凭据表的 SQL
pub const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS kiro_credentials (
    id              BIGSERIAL PRIMARY KEY,
    access_token    TEXT,
    refresh_token   TEXT NOT NULL,
    profile_arn     TEXT,
    expires_at      TIMESTAMPTZ,
    auth_method     VARCHAR(32) DEFAULT 'social',
    client_id       TEXT,
    client_secret   TEXT,
    priority        INTEGER DEFAULT 0,
    region          VARCHAR(32),
    machine_id      VARCHAR(64),
    created_at      TIMESTAMPTZ DEFAULT NOW(),
    updated_at      TIMESTAMPTZ DEFAULT NOW(),
    deleted_at      TIMESTAMPTZ,
    CONSTRAINT valid_auth_method CHECK (auth_method IN ('social', 'idc', 'builder-id'))
);

CREATE INDEX IF NOT EXISTS idx_credentials_priority ON kiro_credentials(priority) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_credentials_updated_at ON kiro_credentials(updated_at);
CREATE INDEX IF NOT EXISTS idx_credentials_expires_at ON kiro_credentials(expires_at) WHERE deleted_at IS NULL;

-- 更新时间触发器
CREATE OR REPLACE FUNCTION update_kiro_credentials_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trigger_kiro_credentials_updated_at ON kiro_credentials;
CREATE TRIGGER trigger_kiro_credentials_updated_at
    BEFORE UPDATE ON kiro_credentials
    FOR EACH ROW
    EXECUTE FUNCTION update_kiro_credentials_updated_at();
"#;
