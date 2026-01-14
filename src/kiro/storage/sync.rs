//! 凭据同步管理器
//!
//! 定时检查存储后端的凭据变更，并通知监听器

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::interval;

use crate::kiro::model::credentials::KiroCredentials;

use super::traits::CredentialStorage;

/// 凭据变更事件
#[derive(Debug, Clone)]
pub enum CredentialChangeEvent {
    /// 凭据已重新加载
    Reloaded(Vec<KiroCredentials>),
}

/// 凭据变更回调函数类型
pub type CredentialChangeCallback = Box<dyn Fn(CredentialChangeEvent) + Send + Sync>;

/// 凭据同步管理器
///
/// 定时检查存储后端的凭据变更，并通知监听器
pub struct CredentialSyncManager {
    /// 存储后端
    storage: Arc<dyn CredentialStorage>,
    /// 同步间隔
    sync_interval: Duration,
    /// 是否启用定时同步
    enabled: AtomicBool,
    /// 上次同步时间戳
    last_sync: AtomicI64,
    /// 变更回调
    callbacks: Mutex<Vec<CredentialChangeCallback>>,
}

impl CredentialSyncManager {
    /// 创建同步管理器
    ///
    /// # Arguments
    /// * `storage` - 存储后端
    /// * `sync_interval_secs` - 同步间隔（秒），0 表示禁用定时同步
    pub fn new(storage: Arc<dyn CredentialStorage>, sync_interval_secs: u64) -> Self {
        Self {
            storage,
            sync_interval: Duration::from_secs(sync_interval_secs),
            enabled: AtomicBool::new(sync_interval_secs > 0),
            last_sync: AtomicI64::new(0),
            callbacks: Mutex::new(Vec::new()),
        }
    }

    /// 添加变更回调
    pub fn add_callback(&self, callback: CredentialChangeCallback) {
        self.callbacks.lock().push(callback);
    }

    /// 启用/禁用定时同步
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// 是否启用定时同步
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// 获取存储后端
    pub fn storage(&self) -> &Arc<dyn CredentialStorage> {
        &self.storage
    }

    /// 手动触发同步
    pub async fn sync_now(&self) -> anyhow::Result<bool> {
        self.check_and_sync().await
    }

    /// 启动定时同步任务
    ///
    /// 返回任务句柄，可用于取消任务
    pub fn start_sync_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let sync_interval = self.sync_interval;

        tokio::spawn(async move {
            if sync_interval.is_zero() {
                tracing::info!("凭据定时同步已禁用");
                return;
            }

            tracing::info!(
                "凭据定时同步已启动，间隔: {} 秒",
                sync_interval.as_secs()
            );

            let mut ticker = interval(sync_interval);

            loop {
                ticker.tick().await;

                if !self.enabled.load(Ordering::Relaxed) {
                    continue;
                }

                match self.check_and_sync().await {
                    Ok(changed) => {
                        if changed {
                            tracing::info!("凭据同步完成，检测到变更");
                        } else {
                            tracing::debug!("凭据同步完成，无变更");
                        }
                    }
                    Err(e) => {
                        tracing::error!("凭据同步失败: {}", e);
                    }
                }
            }
        })
    }

    /// 检查并同步变更
    async fn check_and_sync(&self) -> anyhow::Result<bool> {
        let last_sync = self.last_sync.load(Ordering::Relaxed);

        // 检查是否有变更
        let has_changes = self.storage.has_changes_since(last_sync).await?;

        if !has_changes {
            return Ok(false);
        }

        // 重新加载所有凭据
        let credentials = self.storage.load_all().await?;

        // 更新同步时间
        let now = chrono::Utc::now().timestamp();
        self.last_sync.store(now, Ordering::Relaxed);

        // 通知所有回调
        let event = CredentialChangeEvent::Reloaded(credentials);
        let callbacks = self.callbacks.lock();
        for callback in callbacks.iter() {
            callback(event.clone());
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::storage::FileCredentialStorage;
    use std::io::Write;
    use std::sync::atomic::AtomicUsize;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_sync_manager_creation() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[]").unwrap();

        let storage = Arc::new(FileCredentialStorage::new(file.path(), true));
        let manager = CredentialSyncManager::new(storage, 30);

        assert!(manager.is_enabled());
    }

    #[tokio::test]
    async fn test_sync_manager_disabled() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[]").unwrap();

        let storage = Arc::new(FileCredentialStorage::new(file.path(), true));
        let manager = CredentialSyncManager::new(storage, 0);

        assert!(!manager.is_enabled());
    }

    #[tokio::test]
    async fn test_sync_now() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"[{{"refreshToken": "test", "id": 1}}]"#
        )
        .unwrap();

        let storage = Arc::new(FileCredentialStorage::new(file.path(), true));
        let manager = CredentialSyncManager::new(storage, 30);

        let callback_count = Arc::new(AtomicUsize::new(0));
        let count_clone = callback_count.clone();

        manager.add_callback(Box::new(move |_event| {
            count_clone.fetch_add(1, Ordering::Relaxed);
        }));

        // 首次同步应该触发回调
        let changed = manager.sync_now().await.unwrap();
        assert!(changed);
        assert_eq!(callback_count.load(Ordering::Relaxed), 1);
    }
}
