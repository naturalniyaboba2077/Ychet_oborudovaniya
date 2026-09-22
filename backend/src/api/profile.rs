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
    // Права кладём рядом с каждой группой: профиль показывает роль, а роль
    // в разных организациях одного аккаунта разная. Раньше он выводил её по
    // названию организации — «СтройМонтаж» значило «Владелец» независимо от
    // того, кем человек там был.
    u["workspaces"] = Value::Array(
        wids.into_iter()
            .filter_map(|id| {
                let mut ws = jsn::workspace_json(conn, id)?;
                let rights = merged_rights(conn, uid, id);
                ws.as_object_mut()?.insert("rights".into(), rights);
                Some(ws)
            })
            .collect(),
    );
    // Привязан ли Google — чтобы профиль показывал либо кнопку привязки,
    // либо уже привязанную почту. Сам google_sub наружу не отдаём: клиенту
    // он не нужен, а это идентификатор аккаунта в чужой системе.
    let (email, linked): (Option<String>, bool) = conn
        .query_row(
            "SELECT email, google_sub IS NOT NULL AND google_sub <> '' FROM users WHERE id=?1",
            params![uid],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
        )
        .unwrap_or((None, false));
    u["email"] = json!(email);
    u["googleLinked"] = json!(linked);
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

/// Выход из организации.
///
/// Интерфейс обещал это давно, но кнопка лишь прятала группу на экране: при
/// следующем входе она возвращалась, а человек всё это время оставался в
/// составе и числился ответственным за инструмент.
///
/// Последнего, кто может управлять группой, не выпускаем: группа без
/// управляющего — это каталог, к которому никто не может выдать доступ, и
/// починить это изнутри уже нельзя.
pub(crate) fn profile_leave_workspace(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let ws = i64v(input, "workspaceId").ok_or_else(|| ApiError::bad("workspaceId"))?;
    require_member(conn, uid, ws)?;

    if can_in_workspace(conn, uid, ws, "manageUsers") {
        let mut stmt = conn.prepare(
            "SELECT user_id FROM user_workspaces WHERE workspace_id=?1 AND user_id<>?2",
        )?;
        let others: Vec<i64> = stmt
            .query_map(params![ws, uid], |r| r.get(0))?
            .filter_map(|x| x.ok())
            .collect();
        let another_manager = others
            .iter()
            .any(|&other| can_in_workspace(conn, other, ws, "manageUsers"));
        if !another_manager {
            return Err(ApiError::conflict(
                "Вы единственный, кто управляет этой организацией. Назначьте кого-то ещё руководителем, иначе группа останется без управления",
            ));
        }
    }

    // Инструмент, который за ним числился, возвращаем на склад: иначе он
    // останется закреплённым за человеком, которого в группе уже нет.
    let released = conn.execute(
        "UPDATE items SET responsible_user_id=NULL WHERE workspace_id=?1 AND responsible_user_id=?2",
        params![ws, uid],
    )?;
    conn.execute(
        "UPDATE items SET pending_responsible_id=NULL WHERE workspace_id=?1 AND pending_responsible_id=?2",
        params![ws, uid],
    )?;
    conn.execute(
        "DELETE FROM user_workspaces WHERE user_id=?1 AND workspace_id=?2",
        params![uid, ws],
    )?;
    let name = jsn::user_public(conn, uid)
        .and_then(|u| u["fullName"].as_str().map(str::to_owned))
        .unwrap_or_default();
    ledger::append(
        conn,
        ws,
        uid,
        None,
        "update",
        None,
        None,
        None,
        Some(&format!("{name} вышел из организации")),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    Ok(json!({"ok": true, "workspaceId": ws, "releasedItems": released}))
}

/// Удаление собственного аккаунта.
///
/// Кнопка была, подтверждение паролем спрашивалось, а удаления не
/// происходило: показывалась надпись «Аккаунт удалён (демо)» и человека
/// отправляли на экран входа. При следующем входе всё оказывалось на месте.
///
/// Строку пользователя не стираем: на неё ссылается журнал выдач и
/// передач, а учёт без имён — не учёт. Вместо этого обезличиваем: имя,
/// телефон, почта и привязки уходят, аккаунт закрывается, все входы
/// прекращаются. Для остальных это выглядит как «удалённый пользователь».
pub(crate) fn profile_delete_account(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let hash: Option<String> = conn
        .query_row(
            "SELECT password_hash FROM users WHERE id=?1",
            params![uid],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    // Пароль спрашиваем всегда, когда он есть: удаление необратимо, а
    // чужой незакрытый телефон — самый обычный способ сюда попасть.
    match hash.filter(|h| !h.is_empty()) {
        Some(h) => {
            let given = s(input, "password").unwrap_or_default();
            if !verify_password(&given, &h) {
                return Err(ApiError::unauth("Неверный пароль"));
            }
        }
        None => {
            return Err(ApiError::bad(
                "У аккаунта нет пароля — удалить его может только руководитель организации",
            ))
        }
    }

    // Группы, где он был единственным управляющим, оставлять без
    // управления нельзя — по той же причине, что и при обычном выходе.
    let mut stmt = conn.prepare("SELECT workspace_id FROM user_workspaces WHERE user_id=?1")?;
    let mine: Vec<i64> = stmt
        .query_map(params![uid], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    drop(stmt);
    for ws in &mine {
        if !can_in_workspace(conn, uid, *ws, "manageUsers") {
            continue;
        }
        let mut others_stmt = conn.prepare(
            "SELECT user_id FROM user_workspaces WHERE workspace_id=?1 AND user_id<>?2",
        )?;
        let others: Vec<i64> = others_stmt
            .query_map(params![ws, uid], |r| r.get(0))?
            .filter_map(|x| x.ok())
            .collect();
        drop(others_stmt);
        if !others
            .iter()
            .any(|&other| can_in_workspace(conn, other, *ws, "manageUsers"))
            && !others.is_empty()
        {
            let name: String = conn
                .query_row("SELECT name FROM workspaces WHERE id=?1", params![ws], |r| {
                    r.get(0)
                })
                .unwrap_or_default();
            return Err(ApiError::conflict(format!(
                "В организации «{name}» вы единственный руководитель. Назначьте другого, иначе она останется без управления"
            )));
        }
    }

    for ws in &mine {
        conn.execute(
            "UPDATE items SET responsible_user_id=NULL WHERE workspace_id=?1 AND responsible_user_id=?2",
            params![ws, uid],
        )?;
        conn.execute(
            "UPDATE items SET pending_responsible_id=NULL WHERE workspace_id=?1 AND pending_responsible_id=?2",
            params![ws, uid],
        )?;
    }
    conn.execute(
        "DELETE FROM user_workspaces WHERE user_id=?1",
        params![uid],
    )?;
    // Телефон освобождаем: он уникален, и без этого зарегистрироваться
    // заново с тем же номером стало бы невозможно.
    conn.execute(
        "UPDATE users
         SET status='disabled', full_name='Удалённый пользователь', position=NULL,
             phone='удалён-' || id, email=NULL, google_sub=NULL, avatar_url=NULL,
             password_hash=NULL
         WHERE id=?1",
        params![uid],
    )?;
    conn.execute(
        "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
        params![now(), uid],
    )?;
    Ok(json!({"ok": true, "message": "Аккаунт удалён"}))
}
