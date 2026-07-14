//! Codex Profile 数据访问对象。
//!
//! 本模块仅负责 SQL 执行和行映射，不处理路径、端口、文件或运行时控制。

use crate::app_config::AppType;
use crate::codex_profile::{CodexProfile, CodexProfileRef, CodexProfileRoute};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::{params, Connection, Row};

/// 将 Profile 查询行映射为领域数据。
fn map_codex_profile(row: &Row<'_>) -> rusqlite::Result<CodexProfile> {
    Ok(CodexProfile {
        id: row.get(0)?,
        name: row.get(1)?,
        canonical_home_path: row.get(2)?,
        listen_port: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// 将 Route 查询行映射为领域数据。
fn map_codex_profile_route(row: &Row<'_>) -> rusqlite::Result<CodexProfileRoute> {
    Ok(CodexProfileRoute {
        profile_id: row.get(0)?,
        current_provider_id: row.get(1)?,
        enabled: row.get(2)?,
        live_backup_json: row.get(3)?,
        last_error: row.get(4)?,
        recovery_json: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// 构造统一的 Profile 不存在错误。
fn profile_not_found_error(profile_id: &str) -> AppError {
    AppError::InvalidInput(format!("Codex Profile 不存在: {profile_id}"))
}

/// 确认关联操作的 Profile 主记录存在。
fn ensure_codex_profile_exists(conn: &Connection, profile_id: &str) -> Result<(), AppError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM codex_profiles WHERE id = ?1)",
            [profile_id],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

    if exists {
        Ok(())
    } else {
        Err(profile_not_found_error(profile_id))
    }
}

impl Database {
    /// 按创建顺序列出全部 Codex Profile。
    pub fn list_codex_profiles(&self) -> Result<Vec<CodexProfile>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, canonical_home_path, listen_port, created_at, updated_at
                 FROM codex_profiles
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let profiles = stmt
            .query_map([], map_codex_profile)
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(profiles)
    }

    /// 读取指定 Codex Profile，不存在时返回输入错误。
    pub fn get_codex_profile(&self, profile_id: &str) -> Result<CodexProfile, AppError> {
        let conn = lock_conn!(self.conn);
        match conn.query_row(
            "SELECT id, name, canonical_home_path, listen_port, created_at, updated_at
             FROM codex_profiles WHERE id = ?1",
            [profile_id],
            map_codex_profile,
        ) {
            Ok(profile) => Ok(profile),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(profile_not_found_error(profile_id)),
            Err(error) => Err(AppError::Database(error.to_string())),
        }
    }

    /// 写入一个新的 Codex Profile。
    pub fn insert_codex_profile(&self, profile: &CodexProfile) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO codex_profiles
             (id, name, canonical_home_path, listen_port, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                profile.id,
                profile.name,
                profile.canonical_home_path,
                profile.listen_port,
                profile.created_at,
                profile.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 在同一事务内写入 Profile 与关闭状态的空路由记录。
    pub fn create_codex_profile_with_empty_route(
        &self,
        profile: &CodexProfile,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        tx.execute(
            "INSERT INTO codex_profiles
             (id, name, canonical_home_path, listen_port, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                profile.id,
                profile.name,
                profile.canonical_home_path,
                profile.listen_port,
                profile.created_at,
                profile.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        tx.execute(
            "INSERT INTO codex_profile_routes
             (profile_id, current_provider_id, provider_app_type, enabled, updated_at)
             VALUES (?1, NULL, ?2, 0, ?3)",
            params![profile.id, AppType::Codex.as_str(), profile.updated_at],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 更新已有 Codex Profile 的可持久化字段。
    pub fn update_codex_profile(&self, profile: &CodexProfile) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let changed = conn
            .execute(
                "UPDATE codex_profiles
                 SET name = ?1, canonical_home_path = ?2, listen_port = ?3, updated_at = ?4
                 WHERE id = ?5",
                params![
                    profile.name,
                    profile.canonical_home_path,
                    profile.listen_port,
                    profile.updated_at,
                    profile.id,
                ],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(profile_not_found_error(&profile.id));
        }
        Ok(())
    }

    /// 删除指定 Codex Profile 及其级联关联。
    pub fn delete_codex_profile(&self, profile_id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let changed = conn
            .execute("DELETE FROM codex_profiles WHERE id = ?1", [profile_id])
            .map_err(|e| AppError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(profile_not_found_error(profile_id));
        }
        Ok(())
    }

    /// 获取 Profile 的当前供应商路由；尚未配置时返回 None。
    pub fn get_codex_profile_route(
        &self,
        profile_id: &str,
    ) -> Result<Option<CodexProfileRoute>, AppError> {
        let conn = lock_conn!(self.conn);
        ensure_codex_profile_exists(&conn, profile_id)?;
        match conn.query_row(
            "SELECT profile_id, current_provider_id, enabled, live_backup_json, last_error, recovery_json, updated_at
             FROM codex_profile_routes WHERE profile_id = ?1",
            [profile_id],
            map_codex_profile_route,
        ) {
            Ok(route) => Ok(Some(route)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(AppError::Database(error.to_string())),
        }
    }

    /// 保存 Profile 的当前供应商路由。
    pub fn save_codex_profile_route(&self, route: &CodexProfileRoute) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        ensure_codex_profile_exists(&conn, &route.profile_id)?;
        conn.execute(
            "INSERT INTO codex_profile_routes
             (profile_id, current_provider_id, provider_app_type, enabled, live_backup_json, last_error, recovery_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(profile_id) DO UPDATE SET
                 current_provider_id = excluded.current_provider_id,
                 provider_app_type = excluded.provider_app_type,
                 enabled = excluded.enabled,
                 live_backup_json = excluded.live_backup_json,
                 last_error = excluded.last_error,
                 recovery_json = excluded.recovery_json,
                 updated_at = excluded.updated_at",
            params![
                route.profile_id,
                route.current_provider_id,
                AppType::Codex.as_str(),
                route.enabled,
                route.live_backup_json,
                route.last_error,
                route.recovery_json,
                route.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 按位置读取 Profile 的故障转移供应商 ID。
    pub fn list_codex_profile_failovers(&self, profile_id: &str) -> Result<Vec<String>, AppError> {
        let conn = lock_conn!(self.conn);
        ensure_codex_profile_exists(&conn, profile_id)?;
        let mut stmt = conn
            .prepare(
                "SELECT provider_id FROM codex_profile_failovers
                 WHERE profile_id = ?1 ORDER BY position ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let provider_ids = stmt
            .query_map([profile_id], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(provider_ids)
    }

    /// 用传入顺序完整替换 Profile 的故障转移供应商列表。
    pub fn replace_codex_profile_failovers(
        &self,
        profile_id: &str,
        provider_ids: &[String],
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        ensure_codex_profile_exists(&tx, profile_id)?;
        tx.execute(
            "DELETE FROM codex_profile_failovers WHERE profile_id = ?1",
            [profile_id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;

        for (position, provider_id) in provider_ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO codex_profile_failovers
                 (profile_id, position, provider_id, provider_app_type)
                 VALUES (?1, ?2, ?3, ?4)",
                params![profile_id, position, provider_id, AppType::Codex.as_str()],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 列出当前路由或故障转移列表中使用指定供应商的 Profile。
    pub fn list_codex_provider_profile_refs(
        &self,
        provider_id: &str,
    ) -> Result<Vec<CodexProfileRef>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name FROM codex_profiles
                 WHERE id IN (
                     SELECT profile_id FROM codex_profile_routes
                     WHERE current_provider_id = ?1 AND provider_app_type = ?2
                     UNION
                     SELECT profile_id FROM codex_profile_failovers
                     WHERE provider_id = ?1 AND provider_app_type = ?2
                 )
                 ORDER BY name ASC, id ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let profile_refs = stmt
            .query_map(params![provider_id, AppType::Codex.as_str()], |row| {
                Ok(CodexProfileRef {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(profile_refs)
    }
}
