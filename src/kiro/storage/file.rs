//! 文件凭据存储实现
//!
//! 向后兼容现有的 credentials.json 文件格式

use std::path::PathBuf;

use async_trait::async_trait;

use crate::kiro::model::credentials::{CredentialsConfig, KiroCredentials};

use super::traits::CredentialStorage;

/// 文件凭据存储
///
/// 支持单凭据和多凭据 JSON 格式
pub struct FileCredentialStorage {
    /// 凭据文件路径
    path: PathBuf,
    /// 是否为多凭据格式（数组格式才回写）
    is_multiple_format: bool,
}

impl FileCredentialStorage {
    /// 创建文件存储实例
    ///
    /// # Arguments
    /// * `path` - 凭据文件路径
    /// * `is_multiple_format` - 是否为多凭据格式
    pub fn new(path: impl Into<PathBuf>, is_multiple_format: bool) -> Self {
        Self {
            path: path.into(),
            is_multiple_format,
        }
    }

    /// 从文件加载并自动检测格式
    pub fn from_file(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let config = CredentialsConfig::load(&path)?;
        let is_multiple_format = config.is_multiple();
        Ok(Self {
            path,
            is_multiple_format,
        })
    }

    /// 获取文件路径
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// 是否为多凭据格式
    pub fn is_multiple_format(&self) -> bool {
        self.is_multiple_format
    }
}

#[async_trait]
impl CredentialStorage for FileCredentialStorage {
    async fn load_all(&self) -> anyhow::Result<Vec<KiroCredentials>> {
        // 使用 spawn_blocking 避免阻塞异步运行时
        let path = self.path.clone();
        let credentials = tokio::task::spawn_blocking(move || {
            let config = CredentialsConfig::load(&path)?;
            Ok::<_, anyhow::Error>(config.into_sorted_credentials())
        })
        .await??;

        Ok(credentials)
    }

    async fn save(&self, credential: &KiroCredentials) -> anyhow::Result<()> {
        if !self.is_multiple_format {
            return Ok(()); // 单凭据格式不支持单个保存
        }

        // 加载现有凭据，更新或添加
        let mut credentials = self.load_all().await?;

        if let Some(id) = credential.id {
            if let Some(existing) = credentials.iter_mut().find(|c| c.id == Some(id)) {
                *existing = credential.clone();
            } else {
                credentials.push(credential.clone());
            }
        } else {
            credentials.push(credential.clone());
        }

        self.save_all(&credentials).await
    }

    async fn save_all(&self, credentials: &[KiroCredentials]) -> anyhow::Result<()> {
        if !self.is_multiple_format {
            tracing::debug!("单凭据格式，跳过回写");
            return Ok(());
        }

        let json = serde_json::to_string_pretty(credentials)?;
        let path = self.path.clone();

        tokio::task::spawn_blocking(move || std::fs::write(&path, json))
            .await?
            .map_err(|e| anyhow::anyhow!("写入凭据文件失败: {}", e))?;

        tracing::debug!("已回写凭据到文件: {:?}", self.path);
        Ok(())
    }

    async fn delete(&self, id: u64) -> anyhow::Result<()> {
        if !self.is_multiple_format {
            return Ok(());
        }

        let mut credentials = self.load_all().await?;
        credentials.retain(|c| c.id != Some(id));
        self.save_all(&credentials).await
    }

    fn storage_type(&self) -> &'static str {
        "file"
    }

    fn is_writable(&self) -> bool {
        self.is_multiple_format
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_load_single_credential() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"refreshToken": "test", "authMethod": "social"}}"#
        )
        .unwrap();

        let storage = FileCredentialStorage::from_file(file.path()).unwrap();
        assert!(!storage.is_multiple_format());

        let credentials = storage.load_all().await.unwrap();
        assert_eq!(credentials.len(), 1);
        assert_eq!(credentials[0].refresh_token, Some("test".to_string()));
    }

    #[tokio::test]
    async fn test_load_multiple_credentials() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"[
                {{"refreshToken": "t1", "priority": 1}},
                {{"refreshToken": "t2", "priority": 0}}
            ]"#
        )
        .unwrap();

        let storage = FileCredentialStorage::from_file(file.path()).unwrap();
        assert!(storage.is_multiple_format());

        let credentials = storage.load_all().await.unwrap();
        assert_eq!(credentials.len(), 2);
        // 应按优先级排序
        assert_eq!(credentials[0].refresh_token, Some("t2".to_string()));
        assert_eq!(credentials[1].refresh_token, Some("t1".to_string()));
    }

    #[tokio::test]
    async fn test_save_all_multiple_format() {
        let file = NamedTempFile::new().unwrap();
        let storage = FileCredentialStorage::new(file.path(), true);

        let credentials = vec![
            KiroCredentials {
                id: Some(1),
                refresh_token: Some("t1".to_string()),
                ..Default::default()
            },
            KiroCredentials {
                id: Some(2),
                refresh_token: Some("t2".to_string()),
                ..Default::default()
            },
        ];

        storage.save_all(&credentials).await.unwrap();

        // 重新加载验证
        let loaded = storage.load_all().await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn test_save_all_single_format_skipped() {
        let file = NamedTempFile::new().unwrap();
        let storage = FileCredentialStorage::new(file.path(), false);

        let credentials = vec![KiroCredentials {
            id: Some(1),
            refresh_token: Some("t1".to_string()),
            ..Default::default()
        }];

        // 单凭据格式不回写，应该成功但不写入
        storage.save_all(&credentials).await.unwrap();
    }
}
