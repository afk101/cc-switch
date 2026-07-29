//! 数据库模块测试
//!
//! 包含 Schema 迁移和基本功能的测试。

use super::*;
use crate::app_config::MultiAppConfig;
use crate::codex_profile::{CodexProfile, CodexProfileRoute};
use crate::error::AppError;
use crate::provider::{Provider, ProviderManager};
use indexmap::IndexMap;
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::HashMap;
use tempfile::NamedTempFile;

const LEGACY_SCHEMA_SQL: &str = r#"
    CREATE TABLE providers (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        settings_config TEXT NOT NULL,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE provider_endpoints (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        provider_id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        url TEXT NOT NULL
    );
    CREATE TABLE mcp_servers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        server_config TEXT NOT NULL
    );
    CREATE TABLE prompts (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        content TEXT NOT NULL,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE skills (
        key TEXT PRIMARY KEY,
        installed BOOLEAN NOT NULL DEFAULT 0
    );
    CREATE TABLE skill_repos (
        owner TEXT NOT NULL,
        name TEXT NOT NULL,
        PRIMARY KEY (owner, name)
    );
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT
    );
"#;

// v3.8.x（schema v1）的真实表结构快照：用于验证从 v3.8.* 升级到当前版本的迁移链路
// 参考：tag v3.8.3 的 src-tauri/src/database/schema.rs
const V3_8_SCHEMA_V1_SQL: &str = r#"
    CREATE TABLE providers (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        settings_config TEXT NOT NULL,
        website_url TEXT,
        category TEXT,
        created_at INTEGER,
        sort_index INTEGER,
        notes TEXT,
        icon TEXT,
        icon_color TEXT,
        meta TEXT NOT NULL DEFAULT '{}',
        is_current BOOLEAN NOT NULL DEFAULT 0,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE provider_endpoints (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        provider_id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        url TEXT NOT NULL,
        added_at INTEGER,
        FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
    );
    CREATE TABLE mcp_servers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        server_config TEXT NOT NULL,
        description TEXT,
        homepage TEXT,
        docs TEXT,
        tags TEXT NOT NULL DEFAULT '[]',
        enabled_claude BOOLEAN NOT NULL DEFAULT 0,
        enabled_codex BOOLEAN NOT NULL DEFAULT 0,
        enabled_gemini BOOLEAN NOT NULL DEFAULT 0
    );
    CREATE TABLE prompts (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        content TEXT NOT NULL,
        description TEXT,
        enabled BOOLEAN NOT NULL DEFAULT 1,
        created_at INTEGER,
        updated_at INTEGER,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE skills (
        key TEXT PRIMARY KEY,
        installed BOOLEAN NOT NULL DEFAULT 0,
        installed_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE skill_repos (
        owner TEXT NOT NULL,
        name TEXT NOT NULL,
        branch TEXT NOT NULL DEFAULT 'main',
        enabled BOOLEAN NOT NULL DEFAULT 1,
        PRIMARY KEY (owner, name)
    );
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT
    );
"#;

#[derive(Debug)]
struct ColumnInfo {
    r#type: String,
    notnull: i64,
    default: Option<String>,
}

fn get_column_info(conn: &Connection, table: &str, column: &str) -> ColumnInfo {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\");"))
        .expect("prepare pragma");
    let mut rows = stmt.query([]).expect("query pragma");
    while let Some(row) = rows.next().expect("read row") {
        let column_name: String = row.get(1).expect("name");
        if column_name.eq_ignore_ascii_case(column) {
            return ColumnInfo {
                r#type: row.get::<_, String>(2).expect("type"),
                notnull: row.get::<_, i64>(3).expect("notnull"),
                default: row.get::<_, Option<String>>(4).ok().flatten(),
            };
        }
    }
    panic!("column {table}.{column} not found");
}

fn normalize_default(default: &Option<String>) -> Option<String> {
    default
        .as_ref()
        .map(|s| s.trim_matches('\'').trim_matches('"').to_string())
}

#[test]
fn deleted_default_skill_repo_is_not_restored() {
    let db = Database::memory().expect("create memory db");

    assert_eq!(db.init_default_skill_repos().expect("initialize repos"), 4);
    for repo in db.get_skill_repos().expect("get initialized repos") {
        db.delete_skill_repo(&repo.owner, &repo.name)
            .expect("delete repo");
    }
    assert!(db.get_skill_repos().expect("get deleted repos").is_empty());

    assert_eq!(
        db.init_default_skill_repos().expect("reinitialize repos"),
        0
    );
    assert!(db.get_skill_repos().expect("get repos").is_empty());
}

#[test]
fn existing_skill_repo_selection_is_not_supplemented() {
    let db = Database::memory().expect("create memory db");
    let default_store = crate::services::skill::SkillStore::default();
    db.save_skill_repo(&default_store.repos[0])
        .expect("save existing repo");

    assert_eq!(db.init_default_skill_repos().expect("initialize repos"), 0);
    assert_eq!(db.get_skill_repos().expect("get repos").len(), 1);
    assert!(db
        .get_bool_flag("default_skill_repos_initialized")
        .expect("get initialized flag"));
}

#[test]
fn schema_migration_sets_user_version_when_missing() {
    let conn = Connection::open_in_memory().expect("open memory db");

    Database::create_tables_on_conn(&conn).expect("create tables");
    assert_eq!(
        Database::get_user_version(&conn).expect("read version before"),
        0
    );

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migration");

    assert_eq!(
        Database::get_user_version(&conn).expect("read version after"),
        SCHEMA_VERSION
    );
}

/// 从 v13 升级时，Profile 路由必须获得独立的错误摘要列。
#[test]
fn schema_migration_adds_codex_profile_route_last_error() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        "CREATE TABLE codex_profiles (id TEXT PRIMARY KEY);
         CREATE TABLE codex_profile_routes (
             profile_id TEXT PRIMARY KEY,
             current_provider_id TEXT,
             enabled BOOLEAN NOT NULL DEFAULT 0,
             live_backup_json TEXT,
             updated_at INTEGER NOT NULL DEFAULT 0
         );",
    )
    .expect("create v13 tables");
    Database::set_user_version(&conn, 13).expect("set v13");

    Database::apply_schema_migrations_on_conn(&conn).expect("migrate v13");

    assert_eq!(
        get_column_info(&conn, "codex_profile_routes", "last_error").r#type,
        "TEXT"
    );
    assert_eq!(
        Database::get_user_version(&conn).expect("read version"),
        SCHEMA_VERSION
    );
}

#[test]
fn schema_migration_rejects_future_version() {
    let conn = Connection::open_in_memory().expect("open memory db");
    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::set_user_version(&conn, SCHEMA_VERSION + 1).expect("set future version");

    let err =
        Database::apply_schema_migrations_on_conn(&conn).expect_err("should reject higher version");
    assert!(
        err.to_string().contains("数据库版本过新"),
        "unexpected error: {err}"
    );
}

#[test]
fn schema_migration_adds_missing_columns_for_providers() {
    let conn = Connection::open_in_memory().expect("open memory db");

    // 创建旧版 providers 表，缺少新增列
    conn.execute_batch(LEGACY_SCHEMA_SQL)
        .expect("seed old schema");

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    // 验证关键新增列已补齐
    for (table, column) in [
        ("providers", "meta"),
        ("providers", "is_current"),
        ("provider_endpoints", "added_at"),
        ("mcp_servers", "enabled_gemini"),
        ("prompts", "updated_at"),
        ("skills", "installed_at"),
        ("skill_repos", "enabled"),
    ] {
        assert!(
            Database::has_column(&conn, table, column).expect("check column"),
            "{table}.{column} should exist after migration"
        );
    }

    // 验证 meta 列约束保持一致
    let meta = get_column_info(&conn, "providers", "meta");
    assert_eq!(meta.notnull, 1, "meta should be NOT NULL");
    assert_eq!(
        normalize_default(&meta.default).as_deref(),
        Some("{}"),
        "meta default should be '{{}}'"
    );

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
}

#[test]
fn schema_migration_aligns_column_defaults_and_types() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(LEGACY_SCHEMA_SQL)
        .expect("seed old schema");

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    let is_current = get_column_info(&conn, "providers", "is_current");
    assert_eq!(is_current.r#type, "BOOLEAN");
    assert_eq!(is_current.notnull, 1);
    assert_eq!(normalize_default(&is_current.default).as_deref(), Some("0"));

    let tags = get_column_info(&conn, "mcp_servers", "tags");
    assert_eq!(tags.r#type, "TEXT");
    assert_eq!(tags.notnull, 1);
    assert_eq!(normalize_default(&tags.default).as_deref(), Some("[]"));

    let enabled = get_column_info(&conn, "prompts", "enabled");
    assert_eq!(enabled.r#type, "BOOLEAN");
    assert_eq!(enabled.notnull, 1);
    assert_eq!(normalize_default(&enabled.default).as_deref(), Some("1"));

    let installed_at = get_column_info(&conn, "skills", "installed_at");
    assert_eq!(installed_at.r#type, "INTEGER");
    assert_eq!(installed_at.notnull, 1);
    assert_eq!(
        normalize_default(&installed_at.default).as_deref(),
        Some("0")
    );

    let branch = get_column_info(&conn, "skill_repos", "branch");
    assert_eq!(branch.r#type, "TEXT");
    assert_eq!(normalize_default(&branch.default).as_deref(), Some("main"));

    let skill_repo_enabled = get_column_info(&conn, "skill_repos", "enabled");
    assert_eq!(skill_repo_enabled.r#type, "BOOLEAN");
    assert_eq!(skill_repo_enabled.notnull, 1);
    assert_eq!(
        normalize_default(&skill_repo_enabled.default).as_deref(),
        Some("1")
    );
}

#[test]
fn schema_create_tables_include_pricing_model_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    Database::create_tables_on_conn(&conn).expect("create tables");

    let multiplier = get_column_info(&conn, "proxy_config", "default_cost_multiplier");
    assert_eq!(multiplier.r#type, "TEXT");
    assert_eq!(multiplier.notnull, 1);
    assert_eq!(normalize_default(&multiplier.default).as_deref(), Some("1"));

    let pricing_source = get_column_info(&conn, "proxy_config", "pricing_model_source");
    assert_eq!(pricing_source.r#type, "TEXT");
    assert_eq!(pricing_source.notnull, 1);
    assert_eq!(
        normalize_default(&pricing_source.default).as_deref(),
        Some("response")
    );

    let request_model = get_column_info(&conn, "proxy_request_logs", "request_model");
    assert_eq!(request_model.r#type, "TEXT");
    assert_eq!(request_model.notnull, 0);
}

#[test]
fn schema_migration_v4_adds_pricing_model_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL DEFAULT '{}',
            meta TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE proxy_config (app_type TEXT PRIMARY KEY);
        CREATE TABLE proxy_request_logs (request_id TEXT PRIMARY KEY, model TEXT NOT NULL);
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            enabled_opencode INTEGER NOT NULL DEFAULT 0
        );
        "#,
    )
    .expect("seed v4 schema");

    Database::set_user_version(&conn, 4).expect("set user_version=4");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    let multiplier = get_column_info(&conn, "proxy_config", "default_cost_multiplier");
    assert_eq!(multiplier.r#type, "TEXT");
    assert_eq!(multiplier.notnull, 1);
    assert_eq!(normalize_default(&multiplier.default).as_deref(), Some("1"));

    let pricing_source = get_column_info(&conn, "proxy_config", "pricing_model_source");
    assert_eq!(pricing_source.r#type, "TEXT");
    assert_eq!(pricing_source.notnull, 1);
    assert_eq!(
        normalize_default(&pricing_source.default).as_deref(),
        Some("response")
    );

    let request_model = get_column_info(&conn, "proxy_request_logs", "request_model");
    assert_eq!(request_model.r#type, "TEXT");
    assert_eq!(request_model.notnull, 0);

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
}

#[test]
fn migration_v10_to_v11_rebuilds_rollups_with_request_model_dimension() {
    let conn = Connection::open_in_memory().expect("open memory db");

    // 模拟 v10 形状的 rollup 表（主键不含 request_model）+ 一行历史聚合数据，
    // 以及 v10 形状的明细表（无 pricing_model 列）
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            model TEXT NOT NULL,
            request_model TEXT
        );
        CREATE TABLE usage_daily_rollups (
            date TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model TEXT NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            avg_latency_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_type, provider_id, model)
        );
        INSERT INTO usage_daily_rollups
            (date, app_type, provider_id, model, request_count, success_count,
             input_tokens, output_tokens, total_cost_usd, avg_latency_ms)
        VALUES ('2026-05-01', 'claude', 'p1', 'kimi-k2', 7, 7, 1000, 500, '0.07', 120);
        "#,
    )
    .expect("seed v10 rollup table");

    Database::set_user_version(&conn, 10).expect("set user_version=10");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    // 新列存在且 NOT NULL DEFAULT ''
    let request_model = get_column_info(&conn, "usage_daily_rollups", "request_model");
    assert_eq!(request_model.r#type, "TEXT");
    assert_eq!(request_model.notnull, 1);
    let rollup_pricing_model = get_column_info(&conn, "usage_daily_rollups", "pricing_model");
    assert_eq!(rollup_pricing_model.r#type, "TEXT");
    assert_eq!(rollup_pricing_model.notnull, 1);

    // 明细表补上 pricing_model 列（可空，历史行 NULL）
    let pricing_model = get_column_info(&conn, "proxy_request_logs", "pricing_model");
    assert_eq!(pricing_model.r#type, "TEXT");
    assert_eq!(pricing_model.notnull, 0);

    // 历史行保留，request_model 填 ''（未知）
    let (rm, count, input, cost): (String, i64, i64, String) = conn
        .query_row(
            "SELECT request_model, request_count, input_tokens, total_cost_usd
             FROM usage_daily_rollups WHERE model = 'kimi-k2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("migrated row");
    assert_eq!(rm, "");
    assert_eq!(count, 7);
    assert_eq!(input, 1000);
    assert_eq!(cost, "0.07");

    // 主键包含 request_model：同 model 不同别名可共存
    conn.execute(
        "INSERT INTO usage_daily_rollups
            (date, app_type, provider_id, model, request_model, request_count)
         VALUES ('2026-05-01', 'claude', 'p1', 'kimi-k2', 'claude-sonnet-4-6', 1)",
        [],
    )
    .expect("insert row with same model but different request_model");

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
}

#[test]
fn migrates_v11_to_current_codex_profile_schema() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute("PRAGMA foreign_keys = ON;", [])
        .expect("enable foreign keys");

    Database::create_tables_on_conn(&conn).expect("create v11 tables");
    conn.execute_batch(
        "DROP TABLE codex_profile_mcp_servers;
         DROP TABLE codex_profile_skills;
         DROP TABLE codex_profile_failovers;
         DROP TABLE codex_profile_routes;
         DROP TABLE codex_profiles;
         DROP TABLE proxy_request_logs;
         DROP TABLE session_log_sync;
         DROP TABLE usage_daily_rollups;
         CREATE TABLE proxy_request_logs (
             request_id TEXT PRIMARY KEY,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE session_log_sync (
             file_path TEXT PRIMARY KEY,
             last_modified INTEGER NOT NULL,
             last_line_offset INTEGER NOT NULL DEFAULT 0,
             last_synced_at INTEGER NOT NULL
         );
         CREATE TABLE usage_daily_rollups (
             date TEXT NOT NULL,
             app_type TEXT NOT NULL,
             provider_id TEXT NOT NULL,
             model TEXT NOT NULL,
             request_model TEXT NOT NULL DEFAULT '',
             pricing_model TEXT NOT NULL DEFAULT '',
             request_count INTEGER NOT NULL DEFAULT 0,
             success_count INTEGER NOT NULL DEFAULT 0,
             input_tokens INTEGER NOT NULL DEFAULT 0,
             output_tokens INTEGER NOT NULL DEFAULT 0,
             cache_read_tokens INTEGER NOT NULL DEFAULT 0,
             cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
             total_cost_usd TEXT NOT NULL DEFAULT '0',
             avg_latency_ms INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (date, app_type, provider_id, model, request_model, pricing_model)
         );
         INSERT INTO usage_daily_rollups
             (date, app_type, provider_id, model, request_model, pricing_model,
              request_count, success_count, input_tokens, output_tokens,
              cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms)
         VALUES
             ('2026-07-13', 'codex', 'provider-a', 'gpt-5', '', '',
              7, 6, 700, 350, 70, 0, '0.07', 120);",
    )
    .expect("restore v11 profile and rollup schema");
    Database::set_user_version(&conn, 11).expect("set user_version=11");

    Database::apply_schema_migrations_on_conn(&conn).expect("migrate v11 to current");

    assert_eq!(
        Database::get_user_version(&conn).expect("read version"),
        SCHEMA_VERSION,
        "迁移后应写入当前 user_version"
    );
    for table in [
        "profiles",
        "codex_profiles",
        "codex_profile_routes",
        "codex_profile_failovers",
        "codex_profile_mcp_servers",
        "codex_profile_skills",
    ] {
        assert!(
            Database::table_exists(&conn, table).expect("check table"),
            "{table} 应在 v12 中创建"
        );
    }
    for (table, column) in [
        ("proxy_request_logs", "profile_id"),
        ("session_log_sync", "profile_id"),
        ("proxy_request_logs", "input_token_semantics"),
        ("usage_daily_rollups", "input_token_semantics"),
        ("codex_profile_routes", "live_backup_json"),
        ("codex_profile_routes", "recovery_json"),
    ] {
        assert!(
            Database::has_column(&conn, table, column).expect("check column"),
            "{table}.{column} 应在当前迁移中创建"
        );
    }
    let migrated_rollup: (i64, i64, String) = conn
        .query_row(
            "SELECT request_count, input_tokens, profile_id FROM usage_daily_rollups
             WHERE date = '2026-07-13' AND app_type = 'codex' AND provider_id = 'provider-a'
               AND model = 'gpt-5'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read migrated v11 rollup");
    assert_eq!(
        migrated_rollup,
        (7, 700, String::new()),
        "旧 rollup 的数值应保留，且 Profile 归属应回填为空字符串"
    );

    let profile_id = "profile-a";
    conn.execute(
        "INSERT INTO usage_daily_rollups (
            date, app_type, provider_id, model, request_model, pricing_model, profile_id
        ) VALUES ('2026-07-13', 'codex', 'provider-a', 'gpt-5', '', '', ?1)",
        [profile_id],
    )
    .expect("insert rollup with profile id");
    conn.execute(
        "INSERT INTO usage_daily_rollups (
            date, app_type, provider_id, model, request_model, pricing_model, profile_id
        ) VALUES ('2026-07-13', 'codex', 'provider-a', 'gpt-5', '', '', 'profile-b')",
        [],
    )
    .expect("different profile id should use a distinct rollup key");
}

/// v12 升级后应把旧路由表补齐为 Profile 私有 Live 备份结构。
#[test]
fn migrates_v12_to_v13_profile_route_live_backup_schema() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute("PRAGMA foreign_keys = ON;", [])
        .expect("enable foreign keys");
    Database::create_tables_on_conn(&conn).expect("create current tables");
    conn.execute_batch(
        "DROP TABLE codex_profile_routes;
         CREATE TABLE codex_profile_routes (
             profile_id TEXT PRIMARY KEY,
             current_provider_id TEXT,
             provider_app_type TEXT NOT NULL DEFAULT 'codex'
                 CHECK (provider_app_type = 'codex'),
             enabled BOOLEAN NOT NULL DEFAULT 0,
             updated_at INTEGER NOT NULL DEFAULT 0,
             FOREIGN KEY (profile_id) REFERENCES codex_profiles(id) ON DELETE CASCADE,
             FOREIGN KEY (current_provider_id, provider_app_type)
                 REFERENCES providers(id, app_type) ON DELETE RESTRICT
         );",
    )
    .expect("restore v12 route schema");
    Database::set_user_version(&conn, 12).expect("set user_version=12");

    Database::apply_schema_migrations_on_conn(&conn).expect("migrate v12 to current");

    assert_eq!(
        Database::get_user_version(&conn).expect("read version"),
        SCHEMA_VERSION
    );
    assert!(
        Database::has_column(&conn, "codex_profile_routes", "live_backup_json")
            .expect("check live backup column")
    );
    assert!(
        Database::has_column(&conn, "codex_profile_routes", "recovery_json")
            .expect("check recovery column")
    );
}

#[test]
fn codex_profile_schema_enforces_profile_and_failover_uniqueness() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute("PRAGMA foreign_keys = ON;", [])
        .expect("enable foreign keys");

    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::set_user_version(&conn, 11).expect("set user_version=11");
    Database::apply_schema_migrations_on_conn(&conn).expect("migrate v11 to v12");

    conn.execute(
        "INSERT INTO codex_profiles (id, name, canonical_home_path, listen_port, created_at, updated_at)
         VALUES ('profile-a', '个人', '/Users/test/.codex-a', 16701, 1, 1)",
        [],
    )
    .expect("insert profile");

    conn.execute(
        "INSERT INTO providers (id, app_type, name, settings_config, meta)
         VALUES ('provider-a', 'codex', 'Provider A', '{}', '{}')",
        [],
    )
    .expect("insert failover provider");

    assert!(
        conn.execute(
            "INSERT INTO codex_profiles (id, name, canonical_home_path, listen_port, created_at, updated_at)
             VALUES ('profile-b', '重复路径', '/Users/test/.codex-a', 16702, 1, 1)",
            [],
        )
        .is_err(),
        "canonical_home_path 必须唯一"
    );
    assert!(
        conn.execute(
            "INSERT INTO codex_profiles (id, name, canonical_home_path, listen_port, created_at, updated_at)
             VALUES ('profile-c', '重复端口', '/Users/test/.codex-c', 16701, 1, 1)",
            [],
        )
        .is_err(),
        "listen_port 必须唯一"
    );

    conn.execute(
        "INSERT INTO codex_profile_failovers (profile_id, position, provider_id, provider_app_type)
         VALUES ('profile-a', 0, 'provider-a', 'codex')",
        [],
    )
    .expect("insert first failover");
    assert!(
        conn.execute(
            "INSERT INTO codex_profile_failovers (profile_id, position, provider_id, provider_app_type)
             VALUES ('profile-a', 0, 'provider-b', 'codex')",
            [],
        )
        .is_err(),
        "同一 Profile 的故障转移位置必须唯一"
    );
}

#[test]
fn codex_profile_dao_persists_route_failover_order_and_provider_refs() -> Result<(), AppError> {
    let db = Database::memory().expect("create memory db");
    {
        let conn = lock_conn!(db.conn);
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config, meta)
             VALUES ('provider-a', 'codex', 'Provider A', '{}', '{}'),
                    ('provider-b', 'codex', 'Provider B', '{}', '{}')",
            [],
        )
        .expect("insert providers");
    }

    let profile = CodexProfile {
        id: "profile-a".to_string(),
        name: "工作".to_string(),
        canonical_home_path: "/Users/test/.codex-work".to_string(),
        listen_port: 16701,
        created_at: 1,
        updated_at: 1,
    };
    db.insert_codex_profile(&profile).expect("insert profile");
    assert_eq!(
        db.list_codex_profiles().expect("list profiles"),
        vec![profile.clone()]
    );
    assert_eq!(
        db.get_codex_profile("profile-a").expect("get profile"),
        profile
    );

    let updated_profile = CodexProfile {
        name: "工作更新".to_string(),
        updated_at: 2,
        ..profile.clone()
    };
    db.update_codex_profile(&updated_profile)
        .expect("update profile");
    assert_eq!(
        db.get_codex_profile("profile-a")
            .expect("get updated profile"),
        updated_profile
    );

    let route = CodexProfileRoute {
        profile_id: "profile-a".to_string(),
        current_provider_id: Some("provider-a".to_string()),
        enabled: true,
        live_backup_json: Some("{\"config\":\"profile\"}".to_string()),
        last_error: None,
        recovery_json: None,
        updated_at: 2,
    };
    db.save_codex_profile_route(&route).expect("save route");
    assert_eq!(
        db.get_codex_profile_route("profile-a").expect("get route"),
        Some(route)
    );

    db.replace_codex_profile_failovers(
        "profile-a",
        &["provider-b".to_string(), "provider-a".to_string()],
    )
    .expect("replace failovers");
    assert_eq!(
        db.list_codex_profile_failovers("profile-a")
            .expect("list failovers"),
        vec!["provider-b".to_string(), "provider-a".to_string()]
    );
    assert_eq!(
        db.list_codex_provider_profile_refs("provider-a")
            .expect("list profile refs")
            .into_iter()
            .map(|profile_ref| (profile_ref.id, profile_ref.name))
            .collect::<Vec<_>>(),
        vec![("profile-a".to_string(), "工作更新".to_string())]
    );

    Ok(())
}

#[test]
fn codex_profile_dao_rejects_operations_for_missing_profile() -> Result<(), AppError> {
    let db = Database::memory().expect("create memory db");

    assert!(matches!(
        db.get_codex_profile("missing-profile"),
        Err(AppError::InvalidInput(_))
    ));
    assert!(matches!(
        db.delete_codex_profile("missing-profile"),
        Err(AppError::InvalidInput(_))
    ));

    Ok(())
}

#[test]
fn schema_create_tables_repairs_dev_global_profile_marker() {
    let conn = Connection::open_in_memory().expect("open memory db");

    // 模拟跑过未发布开发版的库：user_version 已是 12（迁移不会再跑），
    // 但 current 标记还是全局 key（现按应用分组）
    conn.execute_batch(
        r#"
        CREATE TABLE profiles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            payload TEXT NOT NULL,
            sort_order INTEGER,
            created_at INTEGER,
            updated_at INTEGER
        );
        INSERT INTO profiles (id, name, payload) VALUES ('p1', 'Project A', '{}');
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        INSERT INTO settings (key, value) VALUES ('current_profile_id', 'p1');
        "#,
    )
    .expect("seed dev v12 shape");
    Database::set_user_version(&conn, 12).expect("set user_version=12");

    Database::create_tables_on_conn(&conn).expect("create tables should repair marker");

    // 全局 current 标记改名为 claude 组标记，旧 key 删除
    let claude_marker: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'current_profile_id_claude'",
            [],
            |row| row.get(0),
        )
        .expect("scoped current marker");
    assert_eq!(claude_marker, "p1");
    let old_marker: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key = 'current_profile_id'",
            [],
            |row| row.get(0),
        )
        .expect("count old marker");
    assert_eq!(old_marker, 0);

    // 修复必须幂等：再跑一遍不应破坏已迁移的标记
    Database::create_tables_on_conn(&conn).expect("repair is idempotent");
    let claude_marker: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'current_profile_id_claude'",
            [],
            |row| row.get(0),
        )
        .expect("scoped current marker survives");
    assert_eq!(claude_marker, "p1");
}

#[test]
fn schema_create_tables_repairs_legacy_proxy_config_singleton_to_per_app() {
    let conn = Connection::open_in_memory().expect("open memory db");

    // 模拟测试版 v2：user_version=2，但 proxy_config 仍是单例结构（无 app_type）
    Database::set_user_version(&conn, 2).expect("set user_version");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_config (
            id INTEGER PRIMARY KEY,
            enabled INTEGER NOT NULL DEFAULT 0,
            listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 5000,
            max_retries INTEGER NOT NULL DEFAULT 3,
            request_timeout INTEGER NOT NULL DEFAULT 300,
            enable_logging INTEGER NOT NULL DEFAULT 1,
            target_app TEXT NOT NULL DEFAULT 'claude',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO proxy_config (id, enabled) VALUES (1, 1);
        "#,
    )
    .expect("seed legacy proxy_config");

    Database::create_tables_on_conn(&conn).expect("create tables should repair proxy_config");

    assert!(
        Database::has_column(&conn, "proxy_config", "app_type").expect("check app_type"),
        "proxy_config should be migrated to per-app structure"
    );

    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM proxy_config", [], |r| r.get(0))
        .expect("count rows");
    assert_eq!(count, 4, "per-app proxy_config should have 4 rows");

    // 新结构下应能按 app_type 查询
    let _: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM proxy_config WHERE app_type = 'claude'",
            [],
            |r| r.get(0),
        )
        .expect("query by app_type");
}

#[test]
fn migration_from_v3_8_schema_v1_to_current_schema_v3() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute("PRAGMA foreign_keys = ON;", [])
        .expect("enable foreign keys");

    // 模拟 v3.8.* 用户的数据库（schema v1）
    conn.execute_batch(V3_8_SCHEMA_V1_SQL)
        .expect("seed v3.8 schema v1");
    Database::set_user_version(&conn, 1).expect("set user_version=1");

    // 插入一条旧版 Provider + Skill（用于验证迁移不会破坏既有数据）
    conn.execute(
        "INSERT INTO providers (
            id, app_type, name, settings_config, website_url, category,
            created_at, sort_index, notes, icon, icon_color, meta, is_current
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            "p1",
            "claude",
            "Test Provider",
            serde_json::to_string(&json!({ "anthropicApiKey": "sk-test" })).unwrap(),
            Option::<String>::None,
            Option::<String>::None,
            Option::<i64>::None,
            Option::<usize>::None,
            Option::<String>::None,
            Option::<String>::None,
            Option::<String>::None,
            "{}",
            1,
        ],
    )
    .expect("seed provider");

    conn.execute(
        "INSERT INTO skills (key, installed, installed_at) VALUES (?1, ?2, ?3)",
        params!["claude:demo-skill", 1, 1700000000i64],
    )
    .expect("seed legacy skill");

    // 按应用启动流程：先 create_tables（补齐新增表），再 apply_schema_migrations（按 user_version 迁移）
    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert_eq!(
        Database::get_user_version(&conn).expect("user_version after migration"),
        SCHEMA_VERSION
    );

    // v1 -> v2：providers 新增字段必须补齐
    for column in [
        "cost_multiplier",
        "limit_daily_usd",
        "limit_monthly_usd",
        "provider_type",
        "in_failover_queue",
    ] {
        assert!(
            Database::has_column(&conn, "providers", column).expect("check column"),
            "providers.{column} should exist after migration"
        );
    }

    // 旧 provider 不应丢失，且新增字段应有默认值
    let provider_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'p1' AND app_type = 'claude'",
            [],
            |r| r.get(0),
        )
        .expect("count providers");
    assert_eq!(provider_count, 1);

    let cost_multiplier: String = conn
        .query_row(
            "SELECT cost_multiplier FROM providers WHERE id = 'p1' AND app_type = 'claude'",
            [],
            |r| r.get(0),
        )
        .expect("read cost_multiplier");
    assert_eq!(cost_multiplier, "1.0");

    // v2 -> v3：skills 表重建为统一结构，并设置 pending 标记（后续由启动时扫描文件系统重建数据）
    assert!(
        Database::has_column(&conn, "skills", "enabled_claude").expect("check skills v3 column"),
        "skills table should be migrated to v3 structure"
    );
    let skills_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM skills", [], |r| r.get(0))
        .expect("count skills");
    assert_eq!(skills_count, 0, "skills table should be rebuilt empty");

    let pending: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'skills_ssot_migration_pending'",
            [],
            |r| r.get(0),
        )
        .ok();
    assert!(
        matches!(pending.as_deref(), Some("true") | Some("1")),
        "skills_ssot_migration_pending should be set after v2->v3 migration"
    );
    let snapshot: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'skills_ssot_migration_snapshot'",
            [],
            |r| r.get(0),
        )
        .ok();
    let snapshot = snapshot.expect("skills migration snapshot should be recorded");
    let snapshot_rows: serde_json::Value =
        serde_json::from_str(&snapshot).expect("parse skills migration snapshot");
    assert!(
        snapshot_rows
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| {
                row.get("directory").and_then(|v| v.as_str()) == Some("demo-skill")
                    && row.get("app_type").and_then(|v| v.as_str()) == Some("claude")
            })),
        "skills migration snapshot should preserve legacy app mapping"
    );

    // v3.9+ 新增：proxy_config 三行 seed 必须存在（否则 UI 会查不到默认值）
    let proxy_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM proxy_config", [], |r| r.get(0))
        .expect("count proxy_config rows");
    assert_eq!(proxy_rows, 4);

    // model_pricing 应具备默认数据（迁移时会 seed）
    let pricing_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_pricing", [], |r| r.get(0))
        .expect("count model_pricing rows");
    assert!(pricing_rows > 0, "model_pricing should be seeded");
}

#[test]
fn schema_dry_run_does_not_write_to_disk() {
    // Create minimal valid config for migration
    let mut apps = HashMap::new();
    apps.insert("claude".to_string(), ProviderManager::default());

    let config = MultiAppConfig {
        version: 2,
        apps,
        mcp: Default::default(),
        prompts: Default::default(),
        skills: Default::default(),
        common_config_snippets: Default::default(),
        claude_common_config_snippet: None,
    };

    // Dry-run should succeed without any file I/O errors
    let result = Database::migrate_from_json_dry_run(&config);
    assert!(
        result.is_ok(),
        "Dry-run should succeed with valid config: {result:?}"
    );
}

#[test]
fn dry_run_validates_schema_compatibility() {
    // Create config with actual provider data
    let mut providers = IndexMap::new();
    providers.insert(
        "test-provider".to_string(),
        Provider {
            id: "test-provider".to_string(),
            name: "Test Provider".to_string(),
            settings_config: json!({
                "anthropicApiKey": "sk-test-123",
            }),
            website_url: None,
            category: None,
            created_at: Some(1234567890),
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        },
    );

    let manager = ProviderManager {
        providers,
        current: "test-provider".to_string(),
    };

    let mut apps = HashMap::new();
    apps.insert("claude".to_string(), manager);

    let config = MultiAppConfig {
        version: 2,
        apps,
        mcp: Default::default(),
        prompts: Default::default(),
        skills: Default::default(),
        common_config_snippets: Default::default(),
        claude_common_config_snippet: None,
    };

    // Dry-run should validate the full migration path
    let result = Database::migrate_from_json_dry_run(&config);
    assert!(
        result.is_ok(),
        "Dry-run should succeed with provider data: {result:?}"
    );
}

#[test]
fn schema_model_pricing_is_seeded_on_init() {
    let db = Database::memory().expect("create memory db");

    let conn = db.conn.lock().expect("lock conn");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_pricing", [], |row| row.get(0))
        .expect("count pricing");

    assert!(
        count > 0,
        "模型定价数据应该在初始化时自动填充，实际数量: {}",
        count
    );

    // 验证包含 Claude 模型
    let claude_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'claude-%'",
            [],
            |row| row.get(0),
        )
        .expect("check claude");
    assert!(
        claude_count > 0,
        "应该包含 Claude 模型定价，实际数量: {}",
        claude_count
    );

    // 验证包含 GPT 模型
    let gpt_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'gpt-%'",
            [],
            |row| row.get(0),
        )
        .expect("check gpt");
    assert!(
        gpt_count > 0,
        "应该包含 GPT 模型定价，实际数量: {}",
        gpt_count
    );

    // 验证包含 Gemini 模型
    let gemini_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'gemini-%'",
            [],
            |row| row.get(0),
        )
        .expect("check gemini");
    assert!(
        gemini_count > 0,
        "应该包含 Gemini 模型定价，实际数量: {}",
        gemini_count
    );
}

#[test]
fn model_pricing_seed_repairs_known_outdated_builtin_prices() {
    let db = Database::memory().expect("create memory db");

    {
        let conn = db.conn.lock().expect("lock conn");
        conn.execute(
            "UPDATE model_pricing
             SET input_cost_per_million = '1.68',
                 output_cost_per_million = '3.36',
                 cache_read_cost_per_million = '0.14',
                 cache_creation_cost_per_million = '0'
             WHERE model_id = 'deepseek-v4-pro'",
            [],
        )
        .expect("restore old DeepSeek price");
        conn.execute(
            "UPDATE model_pricing
             SET input_cost_per_million = '9',
                 output_cost_per_million = '9',
                 cache_read_cost_per_million = '9',
                 cache_creation_cost_per_million = '0'
             WHERE model_id = 'glm-5.1'",
            [],
        )
        .expect("set custom GLM price");
    }

    db.ensure_model_pricing_seeded()
        .expect("ensure pricing seeded");

    let conn = db.conn.lock().expect("lock conn");
    let deepseek: (String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million, cache_read_cost_per_million
             FROM model_pricing WHERE model_id = 'deepseek-v4-pro'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("query DeepSeek price");
    assert_eq!(
        deepseek,
        (
            "0.435".to_string(),
            "0.87".to_string(),
            "0.003625".to_string()
        )
    );

    let glm: (String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million, cache_read_cost_per_million
             FROM model_pricing WHERE model_id = 'glm-5.1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("query GLM price");
    assert_eq!(glm, ("9".to_string(), "9".to_string(), "9".to_string()));
}

#[test]
fn ensure_incremental_auto_vacuum_rebuilds_existing_file_db() {
    let temp = NamedTempFile::new().expect("create temp db file");
    let path = temp.path().to_path_buf();

    let conn = Connection::open(&path).expect("open temp db");
    conn.execute("PRAGMA auto_vacuum = NONE;", [])
        .expect("set none auto_vacuum");
    Database::create_tables_on_conn(&conn).expect("create tables");

    assert_eq!(
        Database::get_auto_vacuum_mode(&conn).expect("auto_vacuum before rebuild"),
        0,
        "existing file db should start with NONE auto_vacuum"
    );

    let rebuilt =
        Database::ensure_incremental_auto_vacuum_on_conn(&conn).expect("enable incremental mode");
    assert!(rebuilt, "existing db should require rebuild via VACUUM");
    drop(conn);

    let reopened = Connection::open(&path).expect("reopen temp db");
    assert_eq!(
        Database::get_auto_vacuum_mode(&reopened).expect("auto_vacuum after rebuild"),
        2,
        "file db should persist INCREMENTAL auto_vacuum after VACUUM rebuild"
    );
}
