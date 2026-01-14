//! 凭据存储抽象层
//!
//! 支持多种存储后端：
//! - 文件存储（默认，向后兼容）
//! - PostgreSQL 存储（可选）
//!
//! # 使用方式
//!
//! ```rust
//! // 文件存储
//! let storage = FileCredentialStorage::new("credentials.json", true);
//!
//! // PostgreSQL 存储（需要启用 postgres feature）
//! #[cfg(feature = "postgres")]
//! let storage = PostgresCredentialStorage::new("postgres://...", "kiro_credentials").await?;
//! ```

mod traits;
mod file;
mod sync;

#[cfg(feature = "postgres")]
mod postgres;

pub use traits::CredentialStorage;
pub use file::FileCredentialStorage;
pub use sync::{CredentialSyncManager, CredentialChangeEvent};

#[cfg(feature = "postgres")]
pub use postgres::PostgresCredentialStorage;
