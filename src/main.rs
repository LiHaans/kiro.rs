mod admin;
mod admin_ui;
mod anthropic;
mod common;
mod http_client;
mod kiro;
mod model;
pub mod token;

use std::sync::Arc;

use clap::Parser;
use kiro::model::credentials::{CredentialsConfig, KiroCredentials};
use kiro::provider::KiroProvider;
use kiro::storage::{CredentialChangeEvent, CredentialSyncManager, CredentialStorage, FileCredentialStorage};
use kiro::token_manager::MultiTokenManager;
use model::arg::Args;
use model::config::Config;

#[tokio::main]
async fn main() {
    // 解析命令行参数
    let args = Args::parse();

    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    // 加载配置
    let config_path = args
        .config
        .unwrap_or_else(|| Config::default_config_path().to_string());
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::error!("加载配置失败: {}", e);
        std::process::exit(1);
    });

    // 获取 API Key
    let api_key = config.api_key.clone().unwrap_or_else(|| {
        tracing::error!("配置文件中未设置 apiKey");
        std::process::exit(1);
    });

    // 构建代理配置
    let proxy_config = config.proxy_url.as_ref().map(|url| {
        let mut proxy = http_client::ProxyConfig::new(url);
        if let (Some(username), Some(password)) = (&config.proxy_username, &config.proxy_password) {
            proxy = proxy.with_auth(username, password);
        }
        proxy
    });

    if proxy_config.is_some() {
        tracing::info!("已配置 HTTP 代理: {}", config.proxy_url.as_ref().unwrap());
    }

    // 根据配置创建存储后端
    let (storage, credentials_list, is_multiple_format): (
        Arc<dyn CredentialStorage>,
        Vec<KiroCredentials>,
        bool,
    ) = match config.credential_storage_type.as_str() {
        #[cfg(feature = "postgres")]
        "postgres" => {
            let pg_config = config.postgres.as_ref().unwrap_or_else(|| {
                tracing::error!("credential_storage_type 为 postgres，但未配置 postgres 连接信息");
                std::process::exit(1);
            });

            tracing::info!("使用 PostgreSQL 存储后端: {}", pg_config.table_name);

            let storage = kiro::storage::PostgresCredentialStorage::new(
                &pg_config.database_url,
                &pg_config.table_name,
                pg_config.max_connections,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::error!("连接 PostgreSQL 失败: {}", e);
                std::process::exit(1);
            });

            let storage = Arc::new(storage);
            let credentials = storage.load_all().await.unwrap_or_else(|e| {
                tracing::error!("从 PostgreSQL 加载凭据失败: {}", e);
                std::process::exit(1);
            });

            (storage as Arc<dyn CredentialStorage>, credentials, true)
        }
        _ => {
            // 默认使用文件存储（向后兼容）
            let credentials_path = args
                .credentials
                .unwrap_or_else(|| KiroCredentials::default_credentials_path().to_string());

            let credentials_config = CredentialsConfig::load(&credentials_path).unwrap_or_else(|e| {
                tracing::error!("加载凭证失败: {}", e);
                std::process::exit(1);
            });

            let is_multiple_format = credentials_config.is_multiple();
            let credentials_list = credentials_config.into_sorted_credentials();

            let storage = Arc::new(FileCredentialStorage::new(&credentials_path, is_multiple_format));

            tracing::info!("使用文件存储后端: {}", credentials_path);

            (storage as Arc<dyn CredentialStorage>, credentials_list, is_multiple_format)
        }
    };

    tracing::info!("已加载 {} 个凭据配置", credentials_list.len());

    // 获取第一个凭据用于日志显示
    let first_credentials = credentials_list.first().cloned().unwrap_or_default();
    tracing::debug!("主凭证: {:?}", first_credentials);

    // 创建 MultiTokenManager
    let mut token_manager = MultiTokenManager::new(
        config.clone(),
        credentials_list,
        proxy_config.clone(),
        None, // 不再直接使用文件路径，改用存储后端
        is_multiple_format,
    )
    .unwrap_or_else(|e| {
        tracing::error!("创建 Token 管理器失败: {}", e);
        std::process::exit(1);
    });

    // 设置存储后端
    token_manager.set_storage(storage.clone());

    let token_manager = Arc::new(token_manager);

    // 创建同步管理器并启动定时同步任务
    let sync_interval = config.credential_sync_interval_secs;
    if sync_interval > 0 {
        let sync_manager = Arc::new(CredentialSyncManager::new(storage.clone(), sync_interval));

        // 添加变更回调，热更新 token_manager
        let tm_for_callback = token_manager.clone();
        sync_manager.add_callback(Box::new(move |event| {
            let CredentialChangeEvent::Reloaded(credentials) = event;
            tm_for_callback.reload_credentials(credentials);
        }));

        // 启动定时同步任务
        let _sync_handle = sync_manager.start_sync_task();
        tracing::info!("凭据定时同步已启动，间隔: {} 秒", sync_interval);
    } else {
        tracing::info!("凭据定时同步已禁用");
    }

    let kiro_provider = KiroProvider::with_proxy(token_manager.clone(), proxy_config.clone());

    // 初始化 count_tokens 配置
    token::init_config(token::CountTokensConfig {
        api_url: config.count_tokens_api_url.clone(),
        api_key: config.count_tokens_api_key.clone(),
        auth_type: config.count_tokens_auth_type.clone(),
        proxy: proxy_config,
    });

    // 构建 Anthropic API 路由（从第一个凭据获取 profile_arn）
    let anthropic_app = anthropic::create_router_with_provider(
        &api_key,
        Some(kiro_provider),
        first_credentials.profile_arn.clone(),
    );

    // 构建 Admin API 路由（如果配置了非空的 admin_api_key）
    // 安全检查：空字符串被视为未配置，防止空 key 绕过认证
    let admin_key_valid = config
        .admin_api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    let app = if let Some(admin_key) = &config.admin_api_key {
        if admin_key.trim().is_empty() {
            tracing::warn!("admin_api_key 配置为空，Admin API 未启用");
            anthropic_app
        } else {
            let admin_service = admin::AdminService::new(token_manager.clone());
            let admin_state = admin::AdminState::new(admin_key, admin_service);
            let admin_app = admin::create_admin_router(admin_state);

            // 创建 Admin UI 路由
            let admin_ui_app = admin_ui::create_admin_ui_router();

            tracing::info!("Admin API 已启用");
            tracing::info!("Admin UI 已启用: /admin");
            anthropic_app
                .nest("/api/admin", admin_app)
                .nest("/admin", admin_ui_app)
        }
    } else {
        anthropic_app
    };

    // 启动服务器
    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("启动 Anthropic API 端点: {}", addr);
    tracing::info!("API Key: {}***", &api_key[..(api_key.len() / 2)]);
    tracing::info!("可用 API:");
    tracing::info!("  GET  /v1/models");
    tracing::info!("  POST /v1/messages");
    tracing::info!("  POST /v1/messages/count_tokens");
    if admin_key_valid {
        tracing::info!("Admin API:");
        tracing::info!("  GET  /api/admin/credentials");
        tracing::info!("  POST /api/admin/credentials/:index/disabled");
        tracing::info!("  POST /api/admin/credentials/:index/priority");
        tracing::info!("  POST /api/admin/credentials/:index/reset");
        tracing::info!("  GET  /api/admin/credentials/:index/balance");
        tracing::info!("Admin UI:");
        tracing::info!("  GET  /admin");
    }

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
