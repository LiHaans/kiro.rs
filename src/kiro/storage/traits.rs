//! 凭据存储 trait 定义

use async_trait::async_trait;

use crate::kiro::model::credentials::KiroCredentials;

/// 凭据存储后端抽象
///
/// 支持多种存储实现：文件、PostgreSQL 等
#[async_trait]
pub trait CredentialStorage: Send + Sync {
    /// 加载所有凭据
    ///
    /// 返回按优先级排序的凭据列表
    async fn load_all(&self) -> anyhow::Result<Vec<KiroCredentials>>;

    /// 保存单个凭据（更新或插入）
    ///
    /// 如果凭据已存在（根据 id），则更新；否则插入新凭据
    async fn save(&self, credential: &KiroCredentials) -> anyhow::Result<()>;

    /// 批量保存凭据
    ///
    /// 替换所有现有凭据
    async fn save_all(&self, credentials: &[KiroCredentials]) -> anyhow::Result<()>;

    /// 删除凭据
    async fn delete(&self, id: u64) -> anyhow::Result<()>;

    /// 获取存储类型名称（用于日志）
    fn storage_type(&self) -> &'static str;

    /// 是否支持写操作
    fn is_writable(&self) -> bool {
        true
    }

    /// 检查是否有变更（用于定时同步）
    ///
    /// 默认实现返回 true，表示总是需要重新加载
    /// PostgreSQL 实现可以通过 updated_at 字段优化
    async fn has_changes_since(&self, _since_timestamp: i64) -> anyhow::Result<bool> {
        Ok(true)
    }
}
