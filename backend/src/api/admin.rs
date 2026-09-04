//! Администрирование: люди, группы, склады, объекты, справочники.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

/// Удаляет рабочее пространство.
///
/// Раньше это был голый DELETE: предметы, история, передачи и членства
/// оставались висеть с идентификатором, которого больше нет, — они пропадали
/// из интерфейса, но занимали место и портили выгрузки. Теперь пространство
/// с содержимым удалить нельзя: историю ТЗ требует хранить, а решать за
/// человека, что её пора стереть, мы не вправе. Пустое убирается вместе со
/// своими справочниками.
pub(crate) fn remove_workspace(conn: &Connection, input: &Value) -> ApiResult {
    let id = i64v(input, "id").unwrap_or(0);
    let items: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE workspace_id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if items > 0 {
        return Err(ApiError::bad(format!(
            "В группе ещё {items} предметов. Перенесите или спишите их, потом удаляйте группу"
        )));
    }
    let history: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM history_entries WHERE workspace_id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if history > 0 {
        return Err(ApiError::bad(
            "В группе есть журнал операций — его нельзя удалять вместе с группой",
        ));
    }
    // Пространство пустое: сносим только то, что без него не имеет смысла.
    for table in [
        "user_workspaces",
        "invites",
        "storages",
        "building_sites",
        "statuses",
        "categories",
        "brands",
        "chat_messages",
    ] {
        let _ = conn.execute(
            &format!("DELETE FROM {table} WHERE workspace_id=?1"),
            params![id],
        );
    }
    conn.execute("DELETE FROM workspaces WHERE id=?1", params![id])?;
    Ok(json!({"ok": true}))
}

pub(crate) fn admin_users(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt = conn.prepare("SELECT user_id FROM user_workspaces WHERE workspace_id=?1")?;
    let ids: Vec<i64> = stmt
        .query_map(params![ws], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(
        ids.into_iter()
            .filter_map(|id| jsn::user_public(conn, id))
            .collect(),
    ))
}

pub(crate) fn admin_user_create(conn: &Connection, input: &Value) -> ApiResult {
    let name = s(input, "fullName").ok_or_else(|| ApiError::bad("fullName"))?;
    let phone = s(input, "phone").ok_or_else(|| ApiError::bad("phone"))?;
    conn.execute(
        "INSERT INTO users (full_name, position, phone, status, role_rights, created_at) VALUES (?1,?2,?3,'invited',?4,?5)",
        params![name, s(input,"position"), phone, db::default_rights().to_string(), now()],
    ).map_err(|e| ApiError::conflict(e.to_string()))?;
    let uid = conn.last_insert_rowid();
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    conn.execute(
        "INSERT INTO user_workspaces (user_id, workspace_id, rights_json) VALUES (?1,?2,?3)",
        params![uid, ws, db::default_rights().to_string()],
    )?;
    jsn::user_public(conn, uid).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn admin_user_update(conn: &Connection, input: &Value, actor: Option<i64>) -> ApiResult {
    if let Some(uid) = actor {
        require_can(conn, uid, "manageUsers")?;
    }
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    conn.execute("UPDATE users SET full_name=COALESCE(?2,full_name), position=COALESCE(?3,position), phone=COALESCE(?4,phone), status=COALESCE(?5,status) WHERE id=?1",
        params![id, s(input,"fullName"), s(input,"position"), s(input,"phone"), s(input,"status")])?;
    if let Some(rr) = input.get("roleRights") {
        if !rr.is_null() {
            let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
            conn.execute(
                "UPDATE user_workspaces SET rights_json=?1 WHERE user_id=?2 AND workspace_id=?3",
                params![rr.to_string(), id, ws],
            )?;
        }
    }
    if let Some(cp) = input.get("checkoutPolicy") {
        if !cp.is_null() {
            conn.execute(
                "UPDATE users SET checkout_policy=?1 WHERE id=?2",
                params![cp.to_string(), id],
            )?;
        }
    }
    jsn::user_public(conn, id).ok_or_else(|| ApiError::not_found("нет"))
}

/// Исключение участника. Историю и подписанные блоки трогать нельзя (ТЗ §7—8):
/// если за человеком что-то числится, он блокируется и выводится из пространства,
/// а не стирается вместе со следами своих операций.
pub(crate) fn admin_user_remove(conn: &Connection, input: &Value, actor: Option<i64>) -> ApiResult {
    let uid = require_user(conn, actor)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    if id == uid {
        return Err(ApiError::bad("Нельзя удалить собственную учётную запись"));
    }
    let exists: i64 =
        conn.query_row("SELECT COUNT(*) FROM users WHERE id=?1", params![id], |r| {
            r.get(0)
        })?;
    if exists == 0 {
        return Err(ApiError::not_found("Пользователь не найден"));
    }
    let traces: i64 = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM history_entries WHERE actor_user_id=?1)
              + (SELECT COUNT(*) FROM items WHERE responsible_user_id=?1)
              + (SELECT COUNT(*) FROM item_holdings WHERE user_id=?1 AND returned_at IS NULL)
              + (SELECT COUNT(*) FROM transfers WHERE from_user_id=?1 OR to_user_id=?1)",
        params![id],
        |r| r.get(0),
    )?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    conn.execute(
        "DELETE FROM user_workspaces WHERE user_id=?1 AND workspace_id=?2",
        params![id, ws],
    )?;
    let other_workspaces: i64 = conn.query_row(
        "SELECT COUNT(*) FROM user_workspaces WHERE user_id=?1",
        params![id],
        |r| r.get(0),
    )?;
    if other_workspaces == 0 {
        conn.execute(
            "UPDATE users SET status='disabled' WHERE id=?1",
            params![id],
        )?;
        conn.execute(
            "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
            params![now(), id],
        )?;
    }
    if traces == 0 && other_workspaces == 0 {
        conn.execute("DELETE FROM users WHERE id=?1", params![id])?;
        return Ok(json!({"ok": true, "deleted": true}));
    }
    Ok(json!({
        "ok": true,
        "deleted": false,
        "disabled": other_workspaces == 0,
        "message": "Участник исключён из пространства; история его операций сохранена"
    }))
}

pub(crate) fn admin_user_invite(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let created = admin_user_create(conn, input)?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let token = Uuid::new_v4().to_string().replace('-', "");
    let expires_at = invite_expiry(input);
    conn.execute(
        "INSERT INTO invites (workspace_id, token, role, created_by, max_uses, expires_at, created_at) VALUES (?1,?2,'member',?3,20,?4,?5)",
        params![ws, token, user_id, expires_at, now()],
    )?;
    Ok(json!({"user": created, "token": token, "expiresAt": expires_at}))
}

pub(crate) fn ws_create(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    conn.execute(
        "INSERT INTO workspaces (name, timezone, internal_id_prefix, comment, created_at, sync_url) VALUES (?1,?2,?3,?4,?5,?6)",
        params![s(input,"name").unwrap_or("Группа".into()), s(input,"timezone").unwrap_or("Europe/Moscow".into()), s(input,"internalIdPrefix").unwrap_or("ВН-".into()), s(input,"comment"), now(), s(input,"syncUrl")],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO user_workspaces (user_id, workspace_id, rights_json) VALUES (?1,?2,?3)",
        params![uid, id, db::owner_rights().to_string()],
    )?;
    seed_workspace_defaults(conn, id, uid)?;
    if let Some(url) = s(input, "syncUrl") {
        crate::sync::add_peer(conn, &url, Some("relay"), None);
    }
    jsn::workspace_json(conn, id).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn ws_update(conn: &Connection, input: &Value) -> ApiResult {
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    conn.execute("UPDATE workspaces SET name=COALESCE(?2,name), timezone=COALESCE(?3,timezone), internal_id_prefix=COALESCE(?4,internal_id_prefix), comment=?5, sync_url=COALESCE(?6,sync_url), require_writeoff_photo=CASE WHEN ?7 THEN ?8 ELSE require_writeoff_photo END WHERE id=?1",
        params![id, s(input,"name"), s(input,"timezone"), s(input,"internalIdPrefix"), s(input,"comment"), s(input,"syncUrl"),
                input.get("requireWriteoffPhoto").is_some(), b(input,"requireWriteoffPhoto").unwrap_or(false) as i64])?;
    if let Some(url) = s(input, "syncUrl") {
        crate::sync::add_peer(conn, &url, Some("relay"), None);
    }
    jsn::workspace_json(conn, id).ok_or_else(|| ApiError::not_found("нет"))
}

/// Срок жизни приглашения по умолчанию — неделя (ТЗ: у приглашения есть срок действия).
pub(crate) const INVITE_DEFAULT_TTL_HOURS: i64 = 168;

pub(crate) const INVITE_MAX_TTL_HOURS: i64 = 24 * 365;

pub(crate) fn invite_expiry(input: &Value) -> String {
    let hours = i64v(input, "expiresInHours")
        .unwrap_or(INVITE_DEFAULT_TTL_HOURS)
        .clamp(1, INVITE_MAX_TTL_HOURS);
    (chrono::Utc::now() + chrono::Duration::hours(hours)).to_rfc3339()
}

/// Должность по умолчанию для участника, вступившего по приглашению с ролью.
pub(crate) fn invite_position(role: &str) -> &'static str {
    match role.trim().to_lowercase().as_str() {
        "owner" | "владелец" => "Владелец",
        "admin" | "администратор" => "Администратор",
        "viewer" | "observer" | "наблюдатель" => "Наблюдатель",
        _ => "Участник",
    }
}

pub(crate) fn ws_create_invite(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let token = Uuid::new_v4().to_string().replace('-', "");
    let role = s(input, "role").unwrap_or_else(|| "member".into());
    let expires_at = invite_expiry(input);
    conn.execute(
        "INSERT INTO invites (workspace_id, token, role, created_by, max_uses, expires_at, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![ws, token, role, user_id, i64v(input,"maxUses").unwrap_or(20), expires_at, now()],
    )?;
    let wsj = jsn::workspace_json(conn, ws).unwrap_or(json!({}));
    Ok(json!({
        "token": token,
        "workspaceId": ws,
        "role": role,
        "expiresAt": expires_at,
        "workspace": wsj,
        "payload": {
            "v": 1,
            "t": "join",
            "ws": ws,
            "token": token,
            "role": role,
            "exp": expires_at,
            "name": wsj.get("name"),
            "server": wsj.get("syncUrl")
        }
    }))
}

pub(crate) fn ws_invites(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt = conn.prepare("SELECT id, token, role, max_uses, used_count, revoked, created_at, expires_at FROM invites WHERE workspace_id=?1 AND revoked=0 ORDER BY id DESC")?;
    let rows: Vec<Value> = stmt
        .query_map(params![ws], |r| {
            let invite = Invite {
                id: r.get(0)?,
                workspace_id: ws,
                role: r.get(2)?,
                max_uses: r.get(3)?,
                used_count: r.get(4)?,
                revoked: r.get(5)?,
                expires_at: r.get(7)?,
            };
            Ok(json!({
                "id": invite.id, "token": r.get::<_, String>(1)?, "role": invite.role,
                "maxUses": invite.max_uses, "usedCount": invite.used_count,
                "revoked": invite.revoked != 0, "createdAt": r.get::<_, String>(6)?,
                "expiresAt": invite.expires_at,
                "expired": invite.is_expired(),
                "usable": ensure_invite_usable(&invite).is_ok(),
            }))
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(rows))
}

pub(crate) fn storages_list(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt = conn.prepare("SELECT id FROM storages WHERE workspace_id=?1")?;
    let ids: Vec<i64> = stmt
        .query_map(params![ws], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    let mut out = Vec::new();
    for id in ids {
        let mut v = jsn::storage_obj(conn, Some(id));
        if let Some(uid) = v.get("responsibleUserId").and_then(|x| x.as_i64()) {
            v["responsible"] = jsn::user_public(conn, uid).unwrap_or(Value::Null);
        }
        out.push(v);
    }
    Ok(Value::Array(out))
}

pub(crate) fn storage_create(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    conn.execute("INSERT INTO storages (name, responsible_user_id, workspace_id, address) VALUES (?1,?2,?3,?4)",
        params![s(input,"name").unwrap_or("Склад".into()), i64v(input,"responsibleUserId"), ws, s(input,"address")])?;
    Ok(jsn::storage_obj(conn, Some(conn.last_insert_rowid())))
}

pub(crate) fn storage_update(conn: &Connection, input: &Value) -> ApiResult {
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    conn.execute("UPDATE storages SET name=COALESCE(?2,name), responsible_user_id=?3, address=COALESCE(?4,address) WHERE id=?1",
        params![id, s(input,"name"), i64v(input,"responsibleUserId"), s(input,"address")])?;
    Ok(jsn::storage_obj(conn, Some(id)))
}

pub(crate) fn sites_list(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt = conn.prepare("SELECT id, name, responsible_user_id, workspace_id FROM building_sites WHERE workspace_id=?1")?;
    let rows: Vec<Value> = stmt
        .query_map(params![ws], |r| {
            let uid: Option<i64> = r.get(2)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?,
                "responsibleUserId": uid, "workspaceId": r.get::<_, i64>(3)?,
                "responsible": uid.and_then(|i| jsn::user_public(conn, i))
            }))
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(rows))
}

pub(crate) fn site_create(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    conn.execute(
        "INSERT INTO building_sites (name, responsible_user_id, workspace_id) VALUES (?1,?2,?3)",
        params![
            s(input, "name").unwrap_or("Объект".into()),
            i64v(input, "responsibleUserId"),
            ws
        ],
    )?;
    let id = conn.last_insert_rowid();
    Ok(
        json!({"id": id, "name": s(input,"name"), "workspaceId": ws, "responsibleUserId": i64v(input,"responsibleUserId")}),
    )
}

pub(crate) fn site_update(conn: &Connection, input: &Value) -> ApiResult {
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    conn.execute(
        "UPDATE building_sites SET name=COALESCE(?2,name), responsible_user_id=?3 WHERE id=?1",
        params![id, s(input, "name"), i64v(input, "responsibleUserId")],
    )?;
    Ok(json!({"id": id, "name": s(input,"name")}))
}

pub(crate) fn dict_table(kind: &str) -> Result<&'static str, ApiError> {
    match kind {
        "categories" => Ok("categories"),
        "brands" => Ok("brands"),
        "statuses" => Ok("statuses"),
        _ => Err(ApiError::bad("kind")),
    }
}

pub(crate) fn dict_list(conn: &Connection, input: &Value) -> ApiResult {
    let kind = s(input, "kind").unwrap_or_else(|| "categories".into());
    let table = dict_table(&kind)?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let sql = if table == "statuses" {
        format!("SELECT id, name, description, workspace_id, type, slug, color, bg FROM {table} WHERE workspace_id=?1")
    } else {
        format!("SELECT id, name, description, workspace_id, type, NULL, NULL, NULL FROM {table} WHERE workspace_id=?1")
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Value> = stmt
        .query_map(params![ws], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?,
                "description": r.get::<_, Option<String>>(2)?, "workspaceId": r.get::<_, i64>(3)?,
                "type": r.get::<_, String>(4)?, "slug": r.get::<_, Option<String>>(5)?,
                "color": r.get::<_, Option<String>>(6)?, "bg": r.get::<_, Option<String>>(7)?,
            }))
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(rows))
}

pub(crate) fn dict_create(conn: &Connection, input: &Value) -> ApiResult {
    let kind = s(input, "kind").unwrap_or_else(|| "categories".into());
    let table = dict_table(&kind)?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let name = s(input, "name").ok_or_else(|| ApiError::bad("name"))?;
    if table == "statuses" {
        conn.execute("INSERT INTO statuses (name, description, workspace_id, type, slug, color, bg) VALUES (?1,?2,?3,'status',?4,?5,?6)",
            params![name, s(input,"description"), ws, s(input,"slug").unwrap_or("custom".into()), s(input,"color").unwrap_or("#5E629B".into()), s(input,"bg").unwrap_or("#EDEDF7".into())])?;
    } else {
        let ty = if table == "brands" {
            "brand"
        } else {
            "category"
        };
        conn.execute(
            &format!(
                "INSERT INTO {table} (name, description, workspace_id, type) VALUES (?1,?2,?3,?4)"
            ),
            params![name, s(input, "description"), ws, ty],
        )?;
    }
    Ok(json!({"id": conn.last_insert_rowid(), "name": name, "workspaceId": ws}))
}

pub(crate) fn dict_update(conn: &Connection, input: &Value) -> ApiResult {
    let kind = s(input, "kind").unwrap_or_else(|| "categories".into());
    let table = dict_table(&kind)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    conn.execute(
        &format!("UPDATE {table} SET name=COALESCE(?2,name), description=?3 WHERE id=?1"),
        params![id, s(input, "name"), s(input, "description")],
    )?;
    Ok(json!({"id": id, "ok": true}))
}

pub(crate) fn dict_remove(conn: &Connection, input: &Value) -> ApiResult {
    let kind = s(input, "kind").unwrap_or_else(|| "categories".into());
    let table = dict_table(&kind)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    conn.execute(&format!("DELETE FROM {table} WHERE id=?1"), params![id])?;
    Ok(json!({"ok": true}))
}
