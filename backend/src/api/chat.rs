//! Чат группы и резервные копии.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn chat_list(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let _ = require_user(conn, user_id)?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt = conn.prepare("SELECT id, workspace_id, user_id, text, created_at FROM chat_messages WHERE workspace_id=?1 ORDER BY id DESC LIMIT 200")?;
    let mut rows: Vec<Value> = stmt
        .query_map(params![ws], |r| {
            let uid: i64 = r.get(2)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "workspaceId": r.get::<_, i64>(1)?, "userId": uid,
                "text": r.get::<_, String>(3)?, "createdAt": r.get::<_, String>(4)?,
                "user": jsn::user_public(conn, uid)
            }))
        })?
        .filter_map(|x| x.ok())
        .collect();
    rows.reverse();
    Ok(Value::Array(rows))
}

pub(crate) fn chat_send(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let text = s(input, "text").ok_or_else(|| ApiError::bad("Пустое сообщение"))?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    conn.execute(
        "INSERT INTO chat_messages (workspace_id, user_id, text, created_at) VALUES (?1,?2,?3,?4)",
        params![ws, uid, text, now()],
    )?;
    Ok(json!({
        "id": conn.last_insert_rowid(),
        "workspaceId": ws,
        "userId": uid,
        "text": text,
        "createdAt": now(),
        "user": jsn::user_public(conn, uid)
    }))
}

pub(crate) fn backup_export(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "manageWorkspaces")?;
    let password = s(input, "password").ok_or_else(|| ApiError::bad("Пароль архива обязателен"))?;
    let journal = crate::sync::export_journal(conn);
    crate::sync::encrypt_backup(&password, &journal.to_string())
        .map_err(|e| ApiError::bad(e.to_string()))
}

pub(crate) fn backup_import(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "manageWorkspaces")?;
    let password = s(input, "password").ok_or_else(|| ApiError::bad("Пароль архива обязателен"))?;
    let blob = input
        .get("blob")
        .cloned()
        .ok_or_else(|| ApiError::bad("Нет архива"))?;
    let plain =
        crate::sync::decrypt_backup(&password, &blob).map_err(|e| ApiError::bad(e.to_string()))?;
    let journal: Value = serde_json::from_str(&plain).map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(crate::sync::import_journal(conn, &journal))
}
