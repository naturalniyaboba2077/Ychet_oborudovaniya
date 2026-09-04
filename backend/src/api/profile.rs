//! Профиль пользователя и смена пароля.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn profile_get(conn: &Connection, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let mut u = jsn::user_public(conn, uid).ok_or_else(|| ApiError::unauth("нет"))?;
    let mut st = conn.prepare("SELECT workspace_id FROM user_workspaces WHERE user_id=?1")?;
    let wids: Vec<i64> = st
        .query_map(params![uid], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    u["workspaces"] = Value::Array(
        wids.into_iter()
            .filter_map(|id| jsn::workspace_json(conn, id))
            .collect(),
    );
    Ok(u)
}

pub(crate) fn profile_update(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    conn.execute("UPDATE users SET full_name=COALESCE(?2,full_name), position=COALESCE(?3,position), phone=COALESCE(?4,phone), avatar_url=COALESCE(?5,avatar_url) WHERE id=?1",
        params![uid, s(input,"fullName"), s(input,"position"), s(input,"phone"), s(input,"avatarUrl")])?;
    jsn::user_public(conn, uid).ok_or_else(|| ApiError::not_found("нет"))
}

pub(crate) fn profile_password(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let newp = s(input, "newPassword").ok_or_else(|| ApiError::bad("newPassword"))?;
    if newp.chars().count() < 10 {
        return Err(ApiError::bad("Пароль минимум 10 символов"));
    }
    let old = conn
        .query_row(
            "SELECT password_hash FROM users WHERE id=?1",
            params![uid],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    if let Some(h) = old.filter(|x| !x.is_empty()) {
        let cur = s(input, "currentPassword").unwrap_or_default();
        if !verify_password(&cur, &h) {
            return Err(ApiError::unauth("Неверный текущий пароль"));
        }
    }
    conn.execute(
        "UPDATE users SET password_hash=?1 WHERE id=?2",
        params![hash_password(&newp), uid],
    )?;
    conn.execute(
        "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
        params![now(), uid],
    )?;
    Ok(json!({"ok": true, "message": "Пароль изменён"}))
}
