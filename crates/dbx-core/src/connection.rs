use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use mysql_async::prelude::Queryable;
use mysql_async::Row as MysqlRow;

use crate::database_capabilities;
use crate::db;
use crate::db::agent_driver::AgentMethod;
use crate::db::proxy_tunnel::ProxyTunnelManager;
use crate::db::ssh_tunnel::TunnelManager;
use crate::external;
use crate::models::connection::{
    parse_jdbc_host_port, parse_mongo_first_host, rewrite_jdbc_url_host, ConnectionConfig, DatabaseType,
};
use crate::plugins::{PluginDriverSession, PluginRegistry};
use crate::query_cancel::RunningQueries;
use crate::storage::Storage;

pub const JDBC_PLUGIN_NOT_INSTALLED: &str =
    "JDBC plugin is not installed. Install the optional JDBC plugin to use this connection.";

pub fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MysqlMode {
    Normal,
    Bare,
    OceanBaseOracle,
}

pub enum PoolKind {
    Mysql(db::mysql::MySqlPool, MysqlMode),
    Postgres(deadpool_postgres::Pool),
    Sqlite(db::sqlite::SqliteHandle),
    Redis(db::redis_driver::RedisConnection),
    DuckDb(Arc<std::sync::Mutex<duckdb::Connection>>),
    MongoDb(mongodb::Client),
    ClickHouse(db::clickhouse_driver::ChClient),
    SqlServer(Arc<tokio::sync::Mutex<db::sqlserver::SqlServerClient>>),
    Elasticsearch(db::elasticsearch_driver::EsClient),
    Agent(Arc<tokio::sync::Mutex<db::agent_driver::AgentDriverClient>>),
    ExternalTabular(Arc<external::ExternalPool>),
    ExternalDriver { driver_id: String, config: Arc<ConnectionConfig>, session: Arc<PluginDriverSession> },
}

pub struct AppState {
    pub connections: RwLock<HashMap<String, PoolKind>>,
    pub configs: RwLock<HashMap<String, ConnectionConfig>>,
    pub running_queries: RunningQueries,
    pub tunnels: TunnelManager,
    pub proxy_tunnels: ProxyTunnelManager,
    pub storage: Storage,
    pub plugins: PluginRegistry,
    pub agent_manager: crate::agent_manager::AgentManager,
}

pub fn metadata_connection_config(config: &ConnectionConfig) -> ConnectionConfig {
    let mut db_config = config.canonicalized();
    if database_capabilities::is_metadata_connection_scoped(&db_config.db_type) {
        db_config.database = None;
    }
    db_config
}

pub fn database_connection_config(config: &ConnectionConfig, database: Option<&str>) -> ConnectionConfig {
    let mut db_config = if database.is_some() { config.clone() } else { metadata_connection_config(config) };
    if let Some(db) = database {
        if !matches!(
            db_config.db_type,
            DatabaseType::Oracle | DatabaseType::Dameng | DatabaseType::MongoDb | DatabaseType::OceanbaseOracle
        ) {
            db_config.database = Some(db.to_string());
        }
    }
    db_config
}

impl AppState {
    pub fn new(storage: Storage) -> Self {
        Self::new_with_plugin_dir(storage, default_plugin_dir())
    }

    pub fn new_with_plugin_dir(storage: Storage, plugin_dir: PathBuf) -> Self {
        Self::new_with_plugin_dir_and_app_version(storage, plugin_dir, env!("CARGO_PKG_VERSION"))
    }

    pub fn new_with_plugin_dir_and_app_version(
        storage: Storage,
        plugin_dir: PathBuf,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
            running_queries: RunningQueries::default(),
            tunnels: TunnelManager::new(),
            proxy_tunnels: ProxyTunnelManager::new(),
            storage,
            plugins: PluginRegistry::new(plugin_dir),
            agent_manager: crate::agent_manager::AgentManager::new_with_base_dir_and_app_version(
                default_agent_dir(),
                app_version,
            ),
        }
    }

    pub fn jdbc_unavailable_error(&self) -> String {
        match self.plugins.find_driver("jdbc") {
            Ok(Some(_)) => "JDBC plugin is installed, but the connection could not be opened.".to_string(),
            Ok(None) => JDBC_PLUGIN_NOT_INSTALLED.to_string(),
            Err(err) => format!("Failed to inspect JDBC plugin: {err}"),
        }
    }

    pub async fn test_external_driver(&self, driver_id: &str, config: &ConnectionConfig) -> Result<String, String> {
        let params = serde_json::json!({ "connection": config });
        self.plugins.invoke_driver::<serde_json::Value>(driver_id, "testConnection", params).await?;
        Ok("Connection successful".to_string())
    }

    pub async fn external_driver_pool(&self, driver_id: &str, config: &ConnectionConfig) -> Result<PoolKind, String> {
        let session = self.plugins.start_driver_session(driver_id).await?;
        let params = serde_json::json!({ "connection": config });
        session.invoke::<serde_json::Value>("connect", params).await?;
        Ok(PoolKind::ExternalDriver { driver_id: driver_id.to_string(), config: Arc::new(config.clone()), session })
    }

    pub async fn get_or_create_pool(&self, connection_id: &str, database: Option<&str>) -> Result<String, String> {
        self.get_or_create_pool_for_session(connection_id, database, None).await
    }

    pub async fn get_or_create_pool_for_session(
        &self,
        connection_id: &str,
        database: Option<&str>,
        client_session_id: Option<&str>,
    ) -> Result<String, String> {
        let db_type = {
            let configs = self.configs.read().await;
            configs.get(connection_id).map(|c| c.db_type)
        };

        let base_pool_key = base_pool_key_for(db_type, connection_id, database, false);
        let pool_key = session_scoped_pool_key(base_pool_key, client_session_id);

        let conns = self.connections.read().await;
        if conns.contains_key(&pool_key) {
            return Ok(pool_key);
        } else {
            drop(conns);
        }

        let configs = self.configs.read().await;
        let config = configs.get(connection_id).ok_or("Connection config not found")?.clone();
        drop(configs);

        let db_config = database_connection_config(&config, database);

        let (host, port) = self.connection_host_port(connection_id, &db_config).await?;
        probe_connection_endpoint(&db_config, &host, port).await?;
        let url = connection_url_for_endpoint(&db_config, &host, port);
        let connect_timeout = std::time::Duration::from_secs(db_config.effective_connect_timeout_secs());
        let pool = match db_config.db_type {
            DatabaseType::Mysql if db_config.needs_bare_mysql() => {
                PoolKind::Mysql(db::mysql::connect_bare(&url, connect_timeout).await?, MysqlMode::Bare)
            }
            DatabaseType::Mysql => {
                let pool =
                    db::mysql::connect_with_ca_cert(&url, Some(&db_config.ca_cert_path), connect_timeout).await?;
                let mode = detect_ob_oracle_mode(&db_config, &pool).await;
                PoolKind::Mysql(pool, mode)
            }
            DatabaseType::Doris | DatabaseType::StarRocks => {
                PoolKind::Mysql(db::mysql::connect_bare(&url, connect_timeout).await?, MysqlMode::Bare)
            }
            DatabaseType::Postgres | DatabaseType::Redshift | DatabaseType::Gaussdb | DatabaseType::OpenGauss => {
                PoolKind::Postgres(db::postgres::connect(&url, connect_timeout).await?)
            }
            DatabaseType::Sqlite => PoolKind::Sqlite(db::sqlite::connect_path(&expand_tilde(&db_config.host)).await?),
            DatabaseType::Redis => {
                let con = if db_config.uses_redis_cluster() {
                    db::redis_driver::RedisConnection::Cluster(db::redis_driver::connect_cluster(&db_config).await?)
                } else if db_config.uses_redis_sentinel() {
                    db::redis_driver::RedisConnection::Direct(tokio::sync::Mutex::new(
                        db::redis_driver::connect_sentinel(&db_config).await?,
                    ))
                } else {
                    db::redis_driver::RedisConnection::Direct(tokio::sync::Mutex::new(
                        db::redis_driver::connect(&url, connect_timeout).await?,
                    ))
                };
                PoolKind::Redis(con)
            }
            DatabaseType::DuckDb => {
                let con = db::duckdb_driver::connect_path(&expand_tilde(&db_config.host))?;
                {
                    let locked = con.lock().map_err(|e| e.to_string())?;
                    for attached in &db_config.attached_databases {
                        crate::schema::duckdb_attach_database(&locked, &attached.name, &expand_tilde(&attached.path))?;
                    }
                }
                PoolKind::DuckDb(con)
            }
            DatabaseType::MongoDb => {
                let native_err = match db::mongo_driver::connect(&url, connect_timeout).await {
                    Ok(client) => match db::mongo_driver::test_connection(&client, connect_timeout).await {
                        Ok(()) => {
                            self.connections.write().await.insert(pool_key.clone(), PoolKind::MongoDb(client));
                            return Ok(pool_key);
                        }
                        Err(e) => e,
                    },
                    Err(e) => e,
                };
                if native_err.contains("wire version") {
                    log::info!("Native MongoDB driver failed ({native_err}), falling back to agent driver");
                    let connect_params = serde_json::json!({ "connection": agent_connect_params(&db_config, &host, port, db_config.effective_database().unwrap_or("")) });
                    let mut client = self.agent_manager.spawn(&DatabaseType::MongoDb, None).await?;
                    client.connect(connect_params).await.map_err(|err| mongo_legacy_error_with_auth_hint(&err))?;
                    PoolKind::Agent(Arc::new(tokio::sync::Mutex::new(client)))
                } else {
                    return Err(native_err);
                }
            }
            DatabaseType::ClickHouse => {
                let username = if db_config.username.is_empty() { None } else { Some(db_config.username.clone()) };
                let password = if db_config.password.is_empty() { None } else { Some(db_config.password.clone()) };
                let client = db::clickhouse_driver::ChClient::new_with_ca_cert(
                    &url,
                    username,
                    password,
                    Some(&db_config.ca_cert_path),
                    connect_timeout,
                )?;
                db::clickhouse_driver::test_connection(&client, connect_timeout).await?;
                PoolKind::ClickHouse(client)
            }
            DatabaseType::SqlServer => {
                let client = db::sqlserver::connect(
                    &host,
                    port,
                    &db_config.username,
                    &db_config.password,
                    db_config.database.as_deref(),
                    connect_timeout,
                )
                .await?;
                PoolKind::SqlServer(Arc::new(tokio::sync::Mutex::new(client)))
            }
            DatabaseType::Elasticsearch => {
                let accept_invalid_certs = db_config.ssl;
                let client = db::elasticsearch_driver::EsClient::new(
                    &url,
                    Some(&db_config.username),
                    Some(&db_config.password),
                    accept_invalid_certs,
                    connect_timeout,
                );
                db::elasticsearch_driver::test_connection(&client, connect_timeout).await?;
                PoolKind::Elasticsearch(client)
            }
            DatabaseType::Dameng
            | DatabaseType::Kingbase
            | DatabaseType::Highgo
            | DatabaseType::Vastbase
            | DatabaseType::Goldendb
            | DatabaseType::Yashandb
            | DatabaseType::Databricks
            | DatabaseType::SapHana
            | DatabaseType::Teradata
            | DatabaseType::Vertica
            | DatabaseType::Firebird
            | DatabaseType::Exasol
            | DatabaseType::OceanbaseOracle
            | DatabaseType::Gbase
            | DatabaseType::Oracle
            | DatabaseType::H2
            | DatabaseType::Snowflake
            | DatabaseType::Trino
            | DatabaseType::Hive
            | DatabaseType::Db2
            | DatabaseType::Informix
            | DatabaseType::Neo4j
            | DatabaseType::Cassandra
            | DatabaseType::Bigquery
            | DatabaseType::Kylin
            | DatabaseType::Sundb
            | DatabaseType::Tdengine
            | DatabaseType::Access => {
                let connect_params =
                    agent_connect_params(&db_config, &host, port, db_config.effective_database().unwrap_or(""));
                let mut client =
                    self.agent_manager.spawn(&db_config.db_type, db_config.driver_profile.as_deref()).await?;
                let connect_result =
                    client.call_method::<serde_json::Value>(AgentMethod::Connect, connect_params.clone()).await;
                if let Err(err) = connect_result {
                    if let Some(alternate_config) = oracle_alternate_connect_config(&db_config, &err) {
                        log::warn!(
                            "Oracle connect failed with {:?} descriptor: {}. Retrying with {:?} descriptor.",
                            db_config.oracle_connection_type,
                            err,
                            alternate_config.oracle_connection_type
                        );
                        client
                            .call_method::<serde_json::Value>(
                                AgentMethod::Connect,
                                agent_connect_params(
                                    &alternate_config,
                                    &host,
                                    port,
                                    alternate_config.effective_database().unwrap_or(""),
                                ),
                            )
                            .await
                            .map_err(|alternate_err| {
                                format!("{err}\n\nFallback with alternate Oracle descriptor failed: {alternate_err}")
                            })?;
                    } else if should_retry_oracle_with_10g_driver(&db_config, &err) {
                        log::warn!(
                            "Oracle connect failed with profile {:?}: {}. Retrying with oracle-10g profile.",
                            db_config.driver_profile,
                            err
                        );
                        let mut fallback_client =
                            self.agent_manager.spawn(&db_config.db_type, Some("oracle-10g")).await?;
                        fallback_client
                            .call_method::<serde_json::Value>(AgentMethod::Connect, connect_params)
                            .await
                            .map_err(|fallback_err| {
                                format!("{err}\n\nFallback with oracle-10g driver failed: {fallback_err}")
                            })?;
                        client = fallback_client;
                    } else {
                        return Err(err);
                    }
                }
                PoolKind::Agent(Arc::new(tokio::sync::Mutex::new(client)))
            }
            DatabaseType::Jdbc => {
                let mut jdbc_config = db_config.clone();
                if host != config.host || port != config.port {
                    if let Some(ref url) = jdbc_config.connection_string {
                        jdbc_config.connection_string = Some(rewrite_jdbc_url_host(url, &host, port));
                    }
                }
                self.external_driver_pool("jdbc", &jdbc_config).await?
            }
        };

        self.connections.write().await.insert(pool_key.clone(), pool);
        Ok(pool_key)
    }

    pub async fn connection_host_port(
        &self,
        connection_id: &str,
        config: &ConnectionConfig,
    ) -> Result<(String, u16), String> {
        if !config.ssh_enabled || config.ssh_host.is_empty() {
            if config.proxy_enabled && !config.proxy_host.is_empty() {
                if let Some(local_port) = self.proxy_tunnels.local_port(connection_id).await {
                    return Ok(("127.0.0.1".to_string(), local_port));
                }

                let (remote_host, remote_port) = if config.db_type == DatabaseType::MongoDb {
                    config
                        .connection_string
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .and_then(parse_mongo_first_host)
                        .unwrap_or_else(|| (config.host.clone(), config.port))
                } else if config.db_type == DatabaseType::Jdbc {
                    config
                        .connection_string
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .and_then(parse_jdbc_host_port)
                        .unwrap_or_else(|| (config.host.clone(), config.port))
                } else {
                    (config.host.clone(), config.port)
                };

                let local_port = self
                    .proxy_tunnels
                    .start_tunnel(
                        connection_id,
                        config.proxy_type,
                        &config.proxy_host,
                        config.proxy_port,
                        &config.proxy_username,
                        &config.proxy_password,
                        &remote_host,
                        remote_port,
                    )
                    .await?;
                return Ok(("127.0.0.1".to_string(), local_port));
            }
            return Ok((config.host.clone(), config.port));
        }

        if let Some(local_port) = self.tunnels.local_port(connection_id).await {
            return Ok(("127.0.0.1".to_string(), local_port));
        }

        let (remote_host, remote_port) = if config.db_type == DatabaseType::MongoDb {
            config
                .connection_string
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(parse_mongo_first_host)
                .unwrap_or_else(|| (config.host.clone(), config.port))
        } else if config.db_type == DatabaseType::Jdbc {
            config
                .connection_string
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(parse_jdbc_host_port)
                .unwrap_or_else(|| (config.host.clone(), config.port))
        } else {
            (config.host.clone(), config.port)
        };

        let local_port = self
            .tunnels
            .start_tunnel(
                connection_id,
                &config.ssh_host,
                config.ssh_port,
                &config.ssh_user,
                &config.ssh_password,
                &config.ssh_key_path,
                &config.ssh_key_passphrase,
                config.effective_ssh_connect_timeout_secs(),
                &remote_host,
                remote_port,
                config.ssh_expose_lan,
            )
            .await?;

        Ok(("127.0.0.1".to_string(), local_port))
    }

    pub async fn reconnect_pool(&self, connection_id: &str, database: Option<&str>) -> Result<String, String> {
        self.reconnect_pool_for_session(connection_id, database, None).await
    }

    pub async fn reconnect_pool_for_session(
        &self,
        connection_id: &str,
        database: Option<&str>,
        client_session_id: Option<&str>,
    ) -> Result<String, String> {
        let db_type = {
            let configs = self.configs.read().await;
            configs.get(connection_id).map(|c| c.db_type)
        };
        let base_pool_key = base_pool_key_for(db_type, connection_id, database, true);
        let pool_key = session_scoped_pool_key(base_pool_key, client_session_id);
        if self.uses_forwarded_transport(connection_id).await {
            self.remove_connection_pools(connection_id).await;
            self.reset_connection_transport(connection_id).await;
        } else {
            let removed = self.connections.write().await.remove(&pool_key);
            if let Some(pool) = removed {
                close_pool_kind(pool).await;
            }
        }
        self.get_or_create_pool_for_session(connection_id, database, client_session_id).await
    }

    pub async fn close_client_session_pool(
        &self,
        connection_id: &str,
        database: Option<&str>,
        client_session_id: &str,
    ) -> Result<bool, String> {
        let session = normalize_client_session_id(Some(client_session_id));
        let Some(session) = session else {
            return Ok(false);
        };
        let db_type = {
            let configs = self.configs.read().await;
            configs.get(connection_id).map(|c| c.db_type)
        };
        let base_pool_key = base_pool_key_for(db_type, connection_id, database, false);
        let pool_key = session_scoped_pool_key(base_pool_key, Some(&session));
        let removed = self.connections.write().await.remove(&pool_key);
        if let Some(pool) = removed {
            close_pool_kind(pool).await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn duckdb_existing_pool_is_usable_for_config(&self, config: &ConnectionConfig) -> Result<bool, String> {
        if config.db_type != DatabaseType::DuckDb {
            return Ok(false);
        }

        let matches_existing_config = {
            let configs = self.configs.read().await;
            configs.get(&config.id).is_some_and(|existing| {
                existing.db_type == DatabaseType::DuckDb && duckdb_paths_match(&existing.host, &config.host)
            })
        };
        if !matches_existing_config {
            return Ok(false);
        }

        let duckdb_pool = {
            let conns = self.connections.read().await;
            match conns.get(&config.id) {
                Some(PoolKind::DuckDb(con)) => Some(con.clone()),
                _ => None,
            }
        };

        let Some(con) = duckdb_pool else {
            return Ok(false);
        };

        let locked = con.lock().map_err(|e| e.to_string())?;
        locked.execute_batch("SELECT 1;").map_err(|e| format!("DuckDb connection failed: {e}"))?;
        Ok(true)
    }

    pub async fn reset_connection_transport(&self, connection_id: &str) {
        self.tunnels.stop_tunnel(connection_id).await;
        self.proxy_tunnels.stop_tunnel(connection_id).await;
    }

    pub async fn remove_connection_pools(&self, connection_id: &str) {
        let mut conns = self.connections.write().await;
        let keys_to_remove: Vec<String> = conns
            .keys()
            .filter(|k| *k == connection_id || k.starts_with(&format!("{connection_id}:")))
            .cloned()
            .collect();
        let mut removed = Vec::with_capacity(keys_to_remove.len());
        for key in keys_to_remove {
            if let Some(pool) = conns.remove(&key) {
                removed.push(pool);
            }
        }
        drop(conns);
        for pool in removed {
            close_pool_kind(pool).await;
        }
    }

    async fn uses_forwarded_transport(&self, connection_id: &str) -> bool {
        let configs = self.configs.read().await;
        configs.get(connection_id).is_some_and(|config| {
            (config.ssh_enabled && !config.ssh_host.is_empty())
                || (config.proxy_enabled && !config.proxy_host.is_empty())
        })
    }
}

fn normalize_client_session_id(client_session_id: Option<&str>) -> Option<String> {
    client_session_id.map(str::trim).filter(|session| !session.is_empty()).map(|session| session.replace(':', "_"))
}

fn session_scoped_pool_key(base_pool_key: String, client_session_id: Option<&str>) -> String {
    normalize_client_session_id(client_session_id)
        .map(|session| format!("{base_pool_key}:session:{session}"))
        .unwrap_or(base_pool_key)
}

pub async fn close_pool_kind(pool: PoolKind) {
    match pool {
        PoolKind::Mysql(p, _) => {
            let _ = p.disconnect().await;
        }
        PoolKind::Postgres(p) => p.close(),
        PoolKind::Sqlite(_) => {}
        PoolKind::Redis(_) => {}
        PoolKind::DuckDb(con) => {
            crate::db::duckdb_driver::close_connection(con);
        }
        PoolKind::MongoDb(_) => {}
        PoolKind::ClickHouse(_) => {}
        PoolKind::SqlServer(_) => {}
        PoolKind::Elasticsearch(_) => {}
        PoolKind::Agent(client) => {
            let mut client = client.lock().await;
            let _ = client.disconnect().await;
        }
        PoolKind::ExternalTabular(_) => {}
        PoolKind::ExternalDriver { .. } => {}
    }
}

fn base_pool_key_for(
    db_type: Option<DatabaseType>,
    connection_id: &str,
    database: Option<&str>,
    include_elasticsearch_single_pool: bool,
) -> String {
    let is_single_connection_pool = db_type.as_ref().is_some_and(|db_type| {
        let is_single = database_capabilities::is_single_connection_pool(db_type)
            || (include_elasticsearch_single_pool && *db_type == DatabaseType::Elasticsearch);
        is_single && !database_capabilities::is_agent_type(db_type)
    });

    if is_single_connection_pool {
        connection_id.to_string()
    } else {
        match database.map(str::trim).filter(|db| !db.is_empty()) {
            Some(db) => format!("{connection_id}:{db}"),
            None => connection_id.to_string(),
        }
    }
}

fn default_plugin_dir() -> PathBuf {
    default_dbx_dir().join("plugins")
}

fn default_agent_dir() -> PathBuf {
    default_dbx_dir().join("agents")
}

fn default_dbx_dir() -> PathBuf {
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".dbx")
}

pub fn connection_url_for_endpoint(config: &ConnectionConfig, host: &str, port: u16) -> String {
    let normalized = native_postgres_url_config(config);
    let config = normalized.as_ref().unwrap_or(config);
    if host == config.host && port == config.port {
        config.connection_url()
    } else {
        config.connection_url_with_host(host, port)
    }
}

pub fn redacted_connection_url_for_endpoint(config: &ConnectionConfig, host: &str, port: u16) -> String {
    let normalized = native_postgres_url_config(config);
    let config = normalized.as_ref().unwrap_or(config);
    if host == config.host && port == config.port {
        config.redacted_connection_url()
    } else {
        config.redacted_connection_url_with_host(host, port)
    }
}

fn native_postgres_url_config(config: &ConnectionConfig) -> Option<ConnectionConfig> {
    match config.db_type {
        DatabaseType::Gaussdb | DatabaseType::OpenGauss => {
            let mut normalized = config.clone();
            if config.db_type == DatabaseType::Gaussdb {
                let params = normalized.url_params.as_deref().unwrap_or("").trim().trim_start_matches('?');
                if !params.to_lowercase().contains("sslmode=") {
                    normalized.url_params = Some(if params.is_empty() {
                        if config.ssl {
                            "sslmode=require".to_string()
                        } else {
                            "sslmode=disable".to_string()
                        }
                    } else {
                        let sslmode = if config.ssl { "sslmode=require" } else { "sslmode=disable" };
                        format!("{sslmode}&{params}")
                    });
                }
            }
            normalized.db_type = DatabaseType::Postgres;
            Some(normalized)
        }
        _ => None,
    }
}

pub fn agent_connect_params(config: &ConnectionConfig, host: &str, port: u16, database: &str) -> serde_json::Value {
    let agent_database = if config.db_type == DatabaseType::MongoDb {
        mongo_agent_database(config, database)
    } else {
        database.to_string()
    };
    let connection_string = if config.db_type == DatabaseType::MongoDb {
        config.connection_url_with_host(host, port)
    } else if config.db_type == DatabaseType::Oracle {
        oracle_jdbc_connection_string(config, host, port, database)
    } else if matches!(config.db_type, DatabaseType::Kingbase | DatabaseType::Highgo | DatabaseType::Vastbase) {
        postgres_like_agent_jdbc_connection_string(config, host, port, database)
    } else if config.db_type == DatabaseType::SapHana {
        sap_hana_jdbc_connection_string(config, host, port, database)
    } else {
        config.connection_string.as_deref().unwrap_or("").to_string()
    };

    serde_json::json!({
        "host": host,
        "port": port,
        "database": agent_database,
        "username": config.username,
        "password": config.password,
        "url_params": config.url_params.as_deref().unwrap_or(""),
        "connection_string": connection_string,
    })
}

fn mongo_agent_database(config: &ConnectionConfig, database: &str) -> String {
    if let Some(database) = non_empty_database(database) {
        return database.to_string();
    }
    if let Some(database) = config.database.as_deref().and_then(non_empty_database) {
        return database.to_string();
    }
    if let Some(database) = config.connection_string.as_deref().and_then(mongo_uri_database) {
        return database;
    }
    "admin".to_string()
}

fn non_empty_database(database: &str) -> Option<&str> {
    let database = database.trim();
    (!database.is_empty()).then_some(database)
}

fn mongo_uri_database(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("mongodb://").or_else(|| uri.strip_prefix("mongodb+srv://"))?;
    let (_, after_hosts) = rest.split_once('/')?;
    let database = after_hosts.split(['?', '#']).next()?.trim();
    if database.is_empty() {
        return None;
    }
    Some(percent_decode_str(database).decode_utf8_lossy().into_owned())
}

pub fn mongo_legacy_error_with_auth_hint(err: &str) -> String {
    let Some(source_start) = err.find("source='") else {
        return err.to_string();
    };
    if !err.contains("Exception authenticating MongoCredential") || err.contains("Current authentication database:") {
        return err.to_string();
    }
    let source = &err[source_start + "source='".len()..];
    let Some(source_end) = source.find('\'') else {
        return err.to_string();
    };
    let source = &source[..source_end];
    format!(
        "{err}\n\nCurrent authentication database: {source}. If this user was created in admin, set Authentication database to admin or add authSource=admin to URL params."
    )
}

fn oracle_jdbc_connection_string(config: &ConnectionConfig, host: &str, port: u16, database: &str) -> String {
    let database = database.trim();
    if database.is_empty() {
        return config.connection_string.as_deref().unwrap_or("").to_string();
    }

    if config.oracle_connection_type.as_deref() == Some("sid") {
        format!("jdbc:oracle:thin:@{host}:{port}:{database}")
    } else {
        format!("jdbc:oracle:thin:@//{host}:{port}/{database}")
    }
}

fn postgres_like_agent_jdbc_connection_string(
    config: &ConnectionConfig,
    host: &str,
    port: u16,
    database: &str,
) -> String {
    let scheme = match config.db_type {
        DatabaseType::Kingbase => "kingbase8",
        DatabaseType::Highgo => "highgo",
        DatabaseType::Vastbase => "vastbase",
        _ => unreachable!("postgres-like agent JDBC URL requested for {:?}", config.db_type),
    };
    let base = format!("jdbc:{scheme}://{host}:{port}/{}", database.trim());
    append_agent_url_params(base, config.url_params.as_deref())
}

pub fn should_retry_oracle_with_10g_driver(config: &ConnectionConfig, err: &str) -> bool {
    if config.db_type != DatabaseType::Oracle {
        return false;
    }
    if config.driver_profile.as_deref() == Some("oracle-10g") {
        return false;
    }
    let normalized = err.to_lowercase();
    normalized.contains("ora-12541") || normalized.contains("no listener") || err.contains("没有监听程序")
}

pub fn oracle_alternate_connect_config(config: &ConnectionConfig, err: &str) -> Option<ConnectionConfig> {
    if !should_retry_oracle_with_10g_driver(config, err) {
        return None;
    }

    let mut retry = config.clone();
    retry.oracle_connection_type =
        Some(if config.oracle_connection_type.as_deref() == Some("sid") { "service_name" } else { "sid" }.to_string());
    Some(retry)
}

fn sap_hana_jdbc_connection_string(config: &ConnectionConfig, host: &str, port: u16, database: &str) -> String {
    let database = database.trim();
    let params = config.url_params.as_deref().unwrap_or("").trim().trim_start_matches('?');
    let has_database_name = params
        .split(['&', ';'])
        .any(|part| part.split_once('=').map(|(key, _)| key.eq_ignore_ascii_case("databaseName")).unwrap_or(false));

    let mut query_parts = Vec::new();
    if !database.is_empty() && !has_database_name {
        query_parts.push(format!("databaseName={}", utf8_percent_encode(database, NON_ALPHANUMERIC)));
    }
    if !params.is_empty() {
        query_parts.push(params.to_string());
    }

    if query_parts.is_empty() {
        format!("jdbc:sap://{host}:{port}")
    } else {
        format!("jdbc:sap://{host}:{port}/?{}", query_parts.join("&"))
    }
}

fn append_agent_url_params(base: String, params: Option<&str>) -> String {
    let params = params.unwrap_or("").trim().trim_start_matches(['?', '&']);
    if params.is_empty() {
        return base;
    }
    let separator = if base.contains('?') { '&' } else { '?' };
    format!("{base}{separator}{params}")
}

fn duckdb_paths_match(left: &str, right: &str) -> bool {
    let left = expand_tilde(left);
    let right = expand_tilde(right);

    if db::duckdb_driver::is_memory_database_path(&left) || db::duckdb_driver::is_memory_database_path(&right) {
        return left.trim().eq_ignore_ascii_case(right.trim());
    }

    if let (Ok(left_path), Ok(right_path)) = (std::fs::canonicalize(&left), std::fs::canonicalize(&right)) {
        return left_path == right_path;
    }

    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

pub async fn probe_connection_endpoint(config: &ConnectionConfig, host: &str, port: u16) -> Result<(), String> {
    if !uses_tcp_probe(config, host, port) {
        return Ok(());
    }
    let timeout = std::time::Duration::from_secs(config.effective_connect_timeout_secs());
    db::probe_tcp_endpoint(&format!("{:?}", config.db_type), host, port, timeout).await
}

fn uses_tcp_probe(config: &ConnectionConfig, host: &str, port: u16) -> bool {
    if config.db_type == DatabaseType::MongoDb
        && config.connection_string.as_deref().is_some_and(|value| !value.is_empty())
    {
        return false;
    }
    if database_capabilities::skips_tcp_probe(&config.db_type) {
        return false;
    }
    if is_original_hostname_endpoint(config, host, port) {
        return false;
    }
    true
}

fn is_original_hostname_endpoint(config: &ConnectionConfig, host: &str, port: u16) -> bool {
    host == config.host && port == config.port && host.parse::<std::net::IpAddr>().is_err()
}

async fn detect_ob_oracle_mode(config: &ConnectionConfig, pool: &db::mysql::MySqlPool) -> MysqlMode {
    let profile = config.driver_profile.as_deref().unwrap_or("").to_lowercase();
    if !profile.contains("oceanbase") {
        return MysqlMode::Normal;
    }
    let mut conn = match pool.get_conn().await {
        Ok(c) => c,
        Err(_) => return MysqlMode::Normal,
    };
    let result = conn.query_iter("SHOW VARIABLES LIKE 'ob_compatibility_mode'").await;
    let rows: Vec<MysqlRow> = match result {
        Ok(r) => match r.collect_and_drop().await {
            Ok(rows) => rows,
            Err(_) => return MysqlMode::Normal,
        },
        Err(_) => return MysqlMode::Normal,
    };
    match rows.first() {
        Some(row) => {
            let val: String = row.get(1).unwrap_or_default();
            if val.to_lowercase() == "oracle" {
                MysqlMode::OceanBaseOracle
            } else {
                MysqlMode::Normal
            }
        }
        None => MysqlMode::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        agent_connect_params, connection_url_for_endpoint, database_connection_config, metadata_connection_config,
        redacted_connection_url_for_endpoint, uses_tcp_probe, AppState, PoolKind,
    };
    use crate::db;
    use crate::models::connection::{default_connect_timeout_secs, ConnectionConfig, DatabaseType, ProxyType};
    use crate::schema;
    use crate::storage::Storage;

    fn mysql_config(database: Option<&str>) -> ConnectionConfig {
        ConnectionConfig {
            id: "conn".to_string(),
            name: "MySQL".to_string(),
            db_type: DatabaseType::Mysql,
            driver_profile: None,
            driver_label: None,
            url_params: None,
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: "secret".to_string(),
            database: database.map(str::to_string),
            visible_databases: None,
            attached_databases: Vec::new(),
            color: None,
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_user: String::new(),
            ssh_password: String::new(),
            ssh_key_path: String::new(),
            ssh_key_passphrase: String::new(),
            ssh_expose_lan: false,
            ssh_connect_timeout_secs: crate::models::connection::default_ssh_connect_timeout_secs(),
            connect_timeout_secs: default_connect_timeout_secs(),
            query_timeout_secs: crate::models::connection::default_query_timeout_secs(),
            proxy_enabled: false,
            proxy_type: ProxyType::Socks5,
            proxy_host: String::new(),
            proxy_port: 1080,
            proxy_username: String::new(),
            proxy_password: String::new(),
            ssl: false,
            ca_cert_path: String::new(),
            sysdba: false,
            oracle_connection_type: None,
            connection_string: None,
            redis_connection_mode: None,
            redis_sentinel_master: String::new(),
            redis_sentinel_nodes: String::new(),
            redis_sentinel_username: String::new(),
            redis_sentinel_password: String::new(),
            redis_sentinel_tls: false,
            redis_cluster_nodes: String::new(),
            external_config: None,
            jdbc_driver_class: None,
            jdbc_driver_paths: Vec::new(),
            one_time: false,
        }
    }

    #[test]
    fn agent_connect_params_include_url_params() {
        let mut config = mysql_config(Some("testdb"));
        config.username = "informix".to_string();
        config.password = "in4mix".to_string();
        config.url_params = Some("INFORMIXSERVER=informix;CLIENT_LOCALE=en_US.utf8".to_string());

        let params = agent_connect_params(&config, "172.26.128.159", 20013, "testdb");

        assert_eq!(params["host"], "172.26.128.159");
        assert_eq!(params["port"], 20013);
        assert_eq!(params["database"], "testdb");
        assert_eq!(params["username"], "informix");
        assert_eq!(params["password"], "in4mix");
        assert_eq!(params["url_params"], "INFORMIXSERVER=informix;CLIENT_LOCALE=en_US.utf8");
    }

    #[test]
    fn agent_connect_params_build_mongodb_connection_string_from_form_fields() {
        let mut config = mysql_config(Some("RestCloud_V45PUB_Gateway"));
        config.db_type = DatabaseType::MongoDb;
        config.host = "172.22.4.42".to_string();
        config.port = 27017;
        config.username = "mongouser".to_string();
        config.password = "secret".to_string();
        config.url_params = Some("authSource=admin&authMechanism=SCRAM-SHA-1".to_string());

        let params = agent_connect_params(&config, "172.22.4.42", 27017, "RestCloud_V45PUB_Gateway");

        assert_eq!(params["connection_string"], "mongodb://mongouser:secret@172.22.4.42:27017/RestCloud%5FV45PUB%5FGateway?authSource=admin&authMechanism=SCRAM-SHA-1");
    }

    #[test]
    fn agent_connect_params_mongodb_uses_connection_string_database_when_database_is_empty() {
        let mut config = mysql_config(None);
        config.db_type = DatabaseType::MongoDb;
        config.connection_string =
            Some("mongodb://mongouser:secret@172.22.4.42:27017/RestCloud_V45PUB_Gateway?authSource=admin".to_string());

        let params = agent_connect_params(&config, "172.22.4.42", 27017, "");

        assert_eq!(params["database"], "RestCloud_V45PUB_Gateway");
    }

    #[test]
    fn mongo_legacy_auth_error_adds_auth_source_hint() {
        let err = "Agent RPC error: Exception authenticating MongoCredential{mechanism=SCRAM-SHA-1, userName='rwuser', source='gray_lite_twin_fat'}";

        assert_eq!(
            super::mongo_legacy_error_with_auth_hint(err),
            "Agent RPC error: Exception authenticating MongoCredential{mechanism=SCRAM-SHA-1, userName='rwuser', source='gray_lite_twin_fat'}\n\nCurrent authentication database: gray_lite_twin_fat. If this user was created in admin, set Authentication database to admin or add authSource=admin to URL params."
        );
    }

    #[test]
    fn agent_connect_params_build_oracle_service_connection_string() {
        let mut config = mysql_config(Some("ORCLPDB1"));
        config.db_type = DatabaseType::Oracle;
        config.host = "oracle.example.com".to_string();
        config.port = 1521;
        config.username = "system".to_string();
        config.password = "oracle".to_string();
        config.oracle_connection_type = Some("service_name".to_string());

        let params = agent_connect_params(&config, "oracle.example.com", 1521, "ORCLPDB1");

        assert_eq!(params["database"], "ORCLPDB1");
        assert_eq!(params["connection_string"], "jdbc:oracle:thin:@//oracle.example.com:1521/ORCLPDB1");
    }

    #[test]
    fn agent_connect_params_build_postgres_like_agent_connection_string_for_selected_database() {
        let cases = [
            (
                DatabaseType::Kingbase,
                "kingbase.example.com",
                54321,
                "jdbc:kingbase8://kingbase.example.com:54321/platform_face_jgj",
                "jdbc:kingbase8://kingbase.example.com:54321/platform_face_freezer_jgj?sslmode=disable",
            ),
            (
                DatabaseType::Highgo,
                "highgo.example.com",
                5866,
                "jdbc:highgo://highgo.example.com:5866/highgo",
                "jdbc:highgo://highgo.example.com:5866/platform_face_freezer_jgj?sslmode=disable",
            ),
            (
                DatabaseType::Vastbase,
                "vastbase.example.com",
                5432,
                "jdbc:vastbase://vastbase.example.com:5432/postgres",
                "jdbc:vastbase://vastbase.example.com:5432/platform_face_freezer_jgj?sslmode=disable",
            ),
        ];

        for (db_type, host, port, stale_connection_string, expected_connection_string) in cases {
            let mut config = mysql_config(Some("platform_face_jgj"));
            config.db_type = db_type;
            config.host = host.to_string();
            config.port = port;
            config.username = "system".to_string();
            config.password = "secret".to_string();
            config.url_params = Some("sslmode=disable".to_string());
            config.connection_string = Some(stale_connection_string.to_string());

            let params = agent_connect_params(&config, host, port, "platform_face_freezer_jgj");

            assert_eq!(params["database"], "platform_face_freezer_jgj");
            assert_eq!(params["connection_string"], expected_connection_string);
        }
    }

    #[test]
    fn agent_connect_params_build_oracle_sid_connection_string() {
        let mut config = mysql_config(Some("ORCL"));
        config.db_type = DatabaseType::Oracle;
        config.oracle_connection_type = Some("sid".to_string());

        let params = agent_connect_params(&config, "127.0.0.1", 11521, "ORCL");

        assert_eq!(params["connection_string"], "jdbc:oracle:thin:@127.0.0.1:11521:ORCL");
    }

    #[test]
    fn agent_connect_params_preserve_legacy_oracle_configs_as_service_name() {
        let mut config = mysql_config(Some("ORCL"));
        config.db_type = DatabaseType::Oracle;
        config.oracle_connection_type = None;

        let params = agent_connect_params(&config, "127.0.0.1", 11521, "ORCL");

        assert_eq!(params["connection_string"], "jdbc:oracle:thin:@//127.0.0.1:11521/ORCL");
    }

    #[test]
    fn oracle_retry_guard_only_triggers_for_non_10g_listener_errors() {
        let mut config = mysql_config(Some("ORCL"));
        config.db_type = DatabaseType::Oracle;
        config.driver_profile = Some("oracle".to_string());

        assert!(super::should_retry_oracle_with_10g_driver(
            &config,
            "Agent RPC error (-1): ORA-12541: TNS:no listener"
        ));
        assert!(super::should_retry_oracle_with_10g_driver(&config, "host xxx port 1521 中没有监听程序"));

        config.driver_profile = Some("oracle-10g".to_string());
        assert!(!super::should_retry_oracle_with_10g_driver(
            &config,
            "Agent RPC error (-1): ORA-12541: TNS:no listener"
        ));

        config.driver_profile = Some("oracle".to_string());
        assert!(!super::should_retry_oracle_with_10g_driver(
            &config,
            "Agent RPC error (-1): ORA-01017: invalid username/password"
        ));
    }

    #[test]
    fn oracle_listener_errors_can_retry_with_alternate_connect_descriptor() {
        let mut config = mysql_config(Some("ORCL"));
        config.db_type = DatabaseType::Oracle;
        config.driver_profile = Some("oracle".to_string());
        config.oracle_connection_type = Some("service_name".to_string());

        let retry = super::oracle_alternate_connect_config(&config, "Agent RPC error (-1): ORA-12541: TNS:no listener")
            .expect("listener errors should allow alternate descriptor retry");
        assert_eq!(retry.driver_profile.as_deref(), Some("oracle"));
        assert_eq!(retry.oracle_connection_type.as_deref(), Some("sid"));

        let service_retry = super::oracle_alternate_connect_config(
            &retry,
            "Agent RPC error (-1): ORA-12541: host xxx port 1521 中没有监听程序",
        )
        .expect("SID listener errors should allow service-name retry");
        assert_eq!(service_retry.oracle_connection_type.as_deref(), Some("service_name"));
    }

    #[test]
    fn oracle_alternate_descriptor_retry_skips_non_listener_errors_and_10g_profiles() {
        let mut config = mysql_config(Some("ORCL"));
        config.db_type = DatabaseType::Oracle;
        config.driver_profile = Some("oracle".to_string());

        assert!(super::oracle_alternate_connect_config(&config, "ORA-01017: invalid username/password").is_none());

        config.driver_profile = Some("oracle-10g".to_string());
        assert!(super::oracle_alternate_connect_config(&config, "ORA-12541: TNS:no listener").is_none());
    }

    #[test]
    fn agent_connect_params_build_saphana_connection_string_from_database_and_url_params() {
        let mut config = mysql_config(Some("TENANT1"));
        config.db_type = DatabaseType::SapHana;
        config.host = "hana.example.com".to_string();
        config.port = 30013;
        config.username = "SYSTEM".to_string();
        config.password = "secret".to_string();
        config.url_params = Some("encrypt=true".to_string());

        let params = agent_connect_params(&config, "hana.example.com", 30013, "TENANT1");

        assert_eq!(params["database"], "TENANT1");
        assert_eq!(params["connection_string"], "jdbc:sap://hana.example.com:30013/?databaseName=TENANT1&encrypt=true");
    }

    async fn test_app_state() -> (AppState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("dbx-core-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Storage::open(&dir.join("storage.db")).await.unwrap();
        (AppState::new(storage), dir)
    }

    fn live_postgres_like_config(
        db_type: DatabaseType,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
        url_params: Option<&str>,
    ) -> ConnectionConfig {
        let mut config = mysql_config(Some("postgres"));
        config.db_type = db_type;
        config.host = host.to_string();
        config.port = port;
        config.username = username.to_string();
        config.password = password.to_string();
        config.url_params = url_params.map(str::to_string);
        config
    }

    async fn assert_live_postgres_like_query(config: ConnectionConfig) {
        let url = connection_url_for_endpoint(&config, &config.host, config.port);
        let pool = db::postgres::connect(&url, std::time::Duration::from_secs(config.effective_connect_timeout_secs()))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to connect to {:?} at {}:{}: {}", config.db_type, config.host, config.port, err)
            });
        let result =
            db::postgres::execute_query(&pool, "SELECT current_database(), current_schema()").await.unwrap_or_else(
                |err| panic!("failed to query {:?} at {}:{}: {}", config.db_type, config.host, config.port, err),
            );
        assert_eq!(result.rows.len(), 1);
        pool.close();
    }

    #[test]
    fn mysql_metadata_connection_ignores_saved_default_database() {
        let config = mysql_config(Some("app"));

        let metadata = metadata_connection_config(&config);

        assert_eq!(metadata.database, None);
        assert_eq!(metadata.db_type, DatabaseType::Mysql);
    }

    #[test]
    fn mysql_database_connection_keeps_requested_database() {
        let config = mysql_config(Some("app"));

        let scoped = database_connection_config(&config, Some("analytics"));

        assert_eq!(scoped.database.as_deref(), Some("analytics"));
    }

    #[test]
    fn gaussdb_database_connection_keeps_requested_database() {
        let mut config = mysql_config(Some("postgres"));
        config.db_type = DatabaseType::Gaussdb;

        let scoped = database_connection_config(&config, Some("analytics"));

        assert_eq!(scoped.database.as_deref(), Some("analytics"));
    }

    #[test]
    fn gaussdb_endpoint_url_uses_postgres_scheme_for_native_driver() {
        let mut config = mysql_config(Some("postgres"));
        config.db_type = DatabaseType::Gaussdb;
        config.username = "gaussdb".to_string();
        config.password = "secret".to_string();

        assert_eq!(
            connection_url_for_endpoint(&config, &config.host, config.port),
            "postgres://gaussdb:secret@127.0.0.1:3306/postgres?sslmode=disable"
        );
        assert_eq!(
            redacted_connection_url_for_endpoint(&config, &config.host, config.port),
            "postgres://127.0.0.1:3306/postgres?sslmode=disable"
        );
    }

    #[test]
    fn opengauss_endpoint_url_uses_postgres_scheme_for_native_driver() {
        let mut config = mysql_config(Some("postgres"));
        config.db_type = DatabaseType::OpenGauss;
        config.username = "gaussdb".to_string();
        config.password = "secret".to_string();

        assert_eq!(
            connection_url_for_endpoint(&config, &config.host, config.port),
            "postgres://gaussdb:secret@127.0.0.1:3306/postgres"
        );
    }

    #[test]
    fn gaussdb_endpoint_url_keeps_explicit_sslmode() {
        let mut config = mysql_config(Some("postgres"));
        config.db_type = DatabaseType::Gaussdb;
        config.username = "gaussdb".to_string();
        config.password = "secret".to_string();
        config.url_params = Some("sslmode=require&application_name=dbx".to_string());

        assert_eq!(
            connection_url_for_endpoint(&config, &config.host, config.port),
            "postgres://gaussdb:secret@127.0.0.1:3306/postgres?sslmode=require&application_name=dbx"
        );
    }

    #[test]
    fn gaussdb_endpoint_url_uses_require_sslmode_when_tls_enabled() {
        let mut config = mysql_config(Some("postgres"));
        config.db_type = DatabaseType::Gaussdb;
        config.username = "gaussdb".to_string();
        config.password = "secret".to_string();
        config.ssl = true;

        assert_eq!(
            connection_url_for_endpoint(&config, &config.host, config.port),
            "postgres://gaussdb:secret@127.0.0.1:3306/postgres?sslmode=require"
        );
    }

    #[test]
    fn gaussdb_endpoint_url_prepends_default_sslmode_to_custom_params() {
        let mut config = mysql_config(Some("postgres"));
        config.db_type = DatabaseType::Gaussdb;
        config.username = "gaussdb".to_string();
        config.password = "secret".to_string();
        config.url_params = Some("application_name=dbx".to_string());

        assert_eq!(
            connection_url_for_endpoint(&config, &config.host, config.port),
            "postgres://gaussdb:secret@127.0.0.1:3306/postgres?sslmode=disable&application_name=dbx"
        );
    }

    #[test]
    fn mongodb_database_connection_keeps_saved_database_for_auth() {
        let mut config = mysql_config(Some("admin"));
        config.db_type = DatabaseType::MongoDb;

        let scoped = database_connection_config(&config, Some("shop"));

        assert_eq!(scoped.database.as_deref(), Some("admin"));
    }

    #[test]
    fn oracle_database_connection_ignores_requested_database() {
        let mut config = mysql_config(Some("ORCL"));
        config.db_type = DatabaseType::Oracle;

        let scoped = database_connection_config(&config, Some("analytics"));

        assert_eq!(scoped.database.as_deref(), Some("ORCL"));
    }

    #[test]
    fn agent_single_connection_types_keep_database_scoped_pool_keys() {
        assert_eq!(
            super::base_pool_key_for(Some(DatabaseType::Kingbase), "kingbase-conn", Some("app1"), false),
            "kingbase-conn:app1"
        );
        assert_eq!(
            super::base_pool_key_for(Some(DatabaseType::Oracle), "oracle-conn", Some("ORCLPDB1"), false),
            "oracle-conn:ORCLPDB1"
        );
        assert_eq!(
            super::base_pool_key_for(Some(DatabaseType::MongoDb), "mongo-conn", Some("shop"), false),
            "mongo-conn:shop"
        );
    }

    #[test]
    fn non_agent_single_connection_types_still_share_pool_keys() {
        assert_eq!(
            super::base_pool_key_for(Some(DatabaseType::Sqlite), "sqlite-conn", Some("main"), false),
            "sqlite-conn"
        );
        assert_eq!(
            super::base_pool_key_for(Some(DatabaseType::DuckDb), "duckdb-conn", Some("analytics"), false),
            "duckdb-conn"
        );
        assert_eq!(
            super::base_pool_key_for(Some(DatabaseType::Jdbc), "jdbc-conn", Some("analytics"), false),
            "jdbc-conn"
        );
    }

    #[test]
    fn mysql_hostname_connections_skip_tcp_probe() {
        let mut config = mysql_config(Some("app"));
        config.host = "mysql.example.com".to_string();

        assert!(!uses_tcp_probe(&config, "mysql.example.com", 3306));
        assert!(uses_tcp_probe(&config, "192.0.2.10", 3306));
        assert!(uses_tcp_probe(&config, "127.0.0.1", 53306));
    }

    #[test]
    fn native_hostname_connections_skip_tcp_probe() {
        for db_type in [
            DatabaseType::Postgres,
            DatabaseType::Redshift,
            DatabaseType::Redis,
            DatabaseType::ClickHouse,
            DatabaseType::SqlServer,
            DatabaseType::Elasticsearch,
        ] {
            let mut config = mysql_config(Some("app"));
            config.db_type = db_type;
            config.host = "db.example.com".to_string();

            assert!(!uses_tcp_probe(&config, "db.example.com", config.port), "{db_type:?} hostname");
            assert!(uses_tcp_probe(&config, "192.0.2.10", config.port), "{db_type:?} ip");
            assert!(uses_tcp_probe(&config, "127.0.0.1", 54000), "{db_type:?} forwarded");
        }
    }

    #[tokio::test]
    async fn sqlite_get_or_create_pool_initializes_connection_for_web_route() {
        let (state, dir) = test_app_state().await;
        let db_path = dir.join("app.db");
        std::fs::File::create(&db_path).unwrap();
        let mut config = mysql_config(None);
        config.id = "sqlite-conn".to_string();
        config.name = "SQLite".to_string();
        config.db_type = DatabaseType::Sqlite;
        config.host = db_path.to_string_lossy().to_string();
        config.port = 0;

        state.configs.write().await.insert(config.id.clone(), config);

        let pool_key = state.get_or_create_pool("sqlite-conn", None).await.unwrap();
        assert_eq!(pool_key, "sqlite-conn");

        let databases = schema::list_databases_core(&state, "sqlite-conn").await.unwrap();
        assert_eq!(databases.len(), 1);
        assert_eq!(databases[0].name, "main");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn duckdb_existing_pool_can_be_used_for_connection_test() {
        let (state, dir) = test_app_state().await;
        let db_path = dir.join("app.duckdb");
        duckdb::Connection::open(&db_path).unwrap();
        let mut config = mysql_config(None);
        config.id = "duckdb-conn".to_string();
        config.name = "DuckDB".to_string();
        config.db_type = DatabaseType::DuckDb;
        config.host = db_path.to_string_lossy().to_string();
        config.port = 0;

        state.configs.write().await.insert(config.id.clone(), config.clone());
        state.get_or_create_pool("duckdb-conn", None).await.unwrap();

        assert!(state.duckdb_existing_pool_is_usable_for_config(&config).await.unwrap());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn remove_connection_pools_clears_base_and_database_scoped_pools() {
        let (state, dir) = test_app_state().await;
        let pool = crate::db::sqlite::connect_path(":memory:").await.unwrap();

        {
            let mut conns = state.connections.write().await;
            conns.insert("conn".to_string(), PoolKind::Sqlite(pool.clone()));
            conns.insert("conn:analytics".to_string(), PoolKind::Sqlite(pool.clone()));
            conns.insert("conn:session:tab-1".to_string(), PoolKind::Sqlite(pool.clone()));
            conns.insert("conn:analytics:session:tab-1".to_string(), PoolKind::Sqlite(pool.clone()));
            conns.insert("other".to_string(), PoolKind::Sqlite(pool));
        }

        state.remove_connection_pools("conn").await;

        let conns = state.connections.read().await;
        assert!(!conns.contains_key("conn"));
        assert!(!conns.contains_key("conn:analytics"));
        assert!(!conns.contains_key("conn:session:tab-1"));
        assert!(!conns.contains_key("conn:analytics:session:tab-1"));
        assert!(conns.contains_key("other"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn close_client_session_pool_removes_session_scoped_duckdb_pool() {
        let (state, dir) = test_app_state().await;
        let db_path = dir.join("session.duckdb");
        let mut config = mysql_config(None);
        config.id = "duckdb-conn".to_string();
        config.name = "DuckDB".to_string();
        config.db_type = DatabaseType::DuckDb;
        config.host = db_path.to_string_lossy().to_string();
        config.port = 0;

        state.configs.write().await.insert(config.id.clone(), config.clone());
        let pool_key = state
            .get_or_create_pool_for_session("duckdb-conn", Some("main"), Some("tab-1"))
            .await
            .unwrap();
        assert_eq!(pool_key, "duckdb-conn:session:tab-1");

        assert!(state.close_client_session_pool("duckdb-conn", Some("main"), "tab-1").await.unwrap());

        let conns = state.connections.read().await;
        assert!(!conns.contains_key("duckdb-conn:session:tab-1"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn proxy_connection_uses_local_forward_endpoint() {
        let (state, dir) = test_app_state().await;
        let mut config = mysql_config(Some("app"));
        config.proxy_enabled = true;
        config.proxy_host = "127.0.0.1".to_string();
        config.proxy_port = 65000;

        let (host, port) = state.connection_host_port("proxied", &config).await.unwrap();

        assert_eq!(host, "127.0.0.1");
        assert_ne!(port, config.port);
        state.proxy_tunnels.stop_tunnel("proxied").await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    #[ignore = "requires a reachable GaussDB instance via environment variables"]
    async fn live_gaussdb_native_connection_succeeds() {
        let host = std::env::var("DBX_TEST_GAUSSDB_HOST").expect("DBX_TEST_GAUSSDB_HOST not set");
        let port = std::env::var("DBX_TEST_GAUSSDB_PORT")
            .expect("DBX_TEST_GAUSSDB_PORT not set")
            .parse::<u16>()
            .expect("DBX_TEST_GAUSSDB_PORT should be a u16");
        let username = std::env::var("DBX_TEST_GAUSSDB_USER").expect("DBX_TEST_GAUSSDB_USER not set");
        let password = std::env::var("DBX_TEST_GAUSSDB_PASSWORD").expect("DBX_TEST_GAUSSDB_PASSWORD not set");
        let url_params = std::env::var("DBX_TEST_GAUSSDB_URL_PARAMS").ok();

        assert_live_postgres_like_query(live_postgres_like_config(
            DatabaseType::Gaussdb,
            &host,
            port,
            &username,
            &password,
            url_params.as_deref(),
        ))
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a reachable openGauss instance via environment variables"]
    async fn live_opengauss_native_connection_succeeds() {
        let host = std::env::var("DBX_TEST_OPENGAUSS_HOST").expect("DBX_TEST_OPENGAUSS_HOST not set");
        let port = std::env::var("DBX_TEST_OPENGAUSS_PORT")
            .expect("DBX_TEST_OPENGAUSS_PORT not set")
            .parse::<u16>()
            .expect("DBX_TEST_OPENGAUSS_PORT should be a u16");
        let username = std::env::var("DBX_TEST_OPENGAUSS_USER").expect("DBX_TEST_OPENGAUSS_USER not set");
        let password = std::env::var("DBX_TEST_OPENGAUSS_PASSWORD").expect("DBX_TEST_OPENGAUSS_PASSWORD not set");
        let url_params = std::env::var("DBX_TEST_OPENGAUSS_URL_PARAMS").ok();

        assert_live_postgres_like_query(live_postgres_like_config(
            DatabaseType::OpenGauss,
            &host,
            port,
            &username,
            &password,
            url_params.as_deref(),
        ))
        .await;
    }
}
