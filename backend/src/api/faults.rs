//! Неисправности, ремонт и заявки на правку карточек.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn report_fault(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| report_fault_atomic(conn, input, user_id))
}

pub(crate) fn report_fault_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let item_id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let ws = require_item_access(conn, uid, item_id)?;
    require_can_in_workspace(conn, uid, ws, "reportFaults")?;
    let desc = s(input, "description").ok_or_else(|| ApiError::bad("Опишите неисправность"))?;
    let item = jsn::item_json(conn, item_id, false)
        .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
    let severity = s(input, "severity").unwrap_or_else(|| "medium".into());
    conn.execute(
        "INSERT INTO faults (item_id, workspace_id, author_id, severity, description, photo_url, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![item_id, ws, uid, severity, desc, s(input, "photoUrl"), now()],
    )?;
    let fid = conn.last_insert_rowid();
    // Сообщение о неисправности переводит предмет в «На проверке»: решение о
    // ремонте принимает администратор (ТЗ §4, «Неисправность и ремонт»).
    if let Ok(st) = conn.query_row(
        "SELECT id FROM statuses WHERE workspace_id=?1 AND slug='needs-check'",
        params![ws],
        |r| r.get::<_, i64>(0),
    ) {
        conn.execute(
            "UPDATE items SET status_id=?1 WHERE id=?2",
            params![st, item_id],
        )?;
    }
    let title = item["title"].as_str().unwrap_or("");
    ledger::append(
        conn,
        ws,
        uid,
        Some(item_id),
        "update",
        None,
        Some("На проверке"),
        None,
        Some(&format!("Неисправность ({severity}): {desc}")),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    notify_admins(
        conn,
        ws,
        item_id,
        "Неисправность",
        &format!("{title}: {desc}"),
    );
    Ok(json!({"id": fid, "itemId": item_id, "status": "open"}))
}

pub(crate) fn list_faults(conn: &Connection, input: &Value) -> ApiResult {
    let mut sql = String::from("SELECT id, item_id, workspace_id, author_id, severity, description, photo_url, status, resolution, resolver_id, created_at, resolved_at FROM faults WHERE 1=1");
    if let Some(id) = i64v(input, "itemId") {
        sql.push_str(&format!(" AND item_id={id}"));
    } else {
        let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
        sql.push_str(&format!(" AND workspace_id={ws}"));
    }
    sql.push_str(" ORDER BY id DESC LIMIT 200");
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Value> = stmt.query_map([], |r| {
        let author: i64 = r.get(3)?;
        Ok(json!({
            "id": r.get::<_, i64>(0)?, "itemId": r.get::<_, i64>(1)?, "workspaceId": r.get::<_, i64>(2)?,
            "authorId": author, "severity": r.get::<_, String>(4)?, "description": r.get::<_, String>(5)?,
            "photoUrl": r.get::<_, Option<String>>(6)?, "status": r.get::<_, String>(7)?,
            "resolution": r.get::<_, Option<String>>(8)?, "resolverId": r.get::<_, Option<i64>>(9)?,
            "createdAt": r.get::<_, String>(10)?, "resolvedAt": r.get::<_, Option<String>>(11)?,
            "author": jsn::user_public(conn, author)
        }))
    })?.filter_map(|x| x.ok()).collect();
    Ok(Value::Array(rows))
}

pub(crate) fn resolve_fault(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| resolve_fault_atomic(conn, input, user_id))
}

pub(crate) fn resolve_fault_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    let (item_id, ws): (i64, i64) = conn
        .query_row(
            "SELECT item_id, workspace_id FROM faults WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("Неисправность не найдена"))?;
    require_member(conn, uid, ws)?;
    require_can_in_workspace(conn, uid, ws, "editItems")?;
    let status = s(input, "status").unwrap_or_else(|| "resolved".into());
    conn.execute(
        "UPDATE faults SET status=?1, resolution=?2, resolver_id=?3, resolved_at=?4 WHERE id=?5",
        params![status, s(input, "comment"), uid, now(), id],
    )?;
    let slug = if status == "repair" || status == "open" {
        "in-repair"
    } else {
        "in-stock"
    };
    if let Ok(st) = conn.query_row(
        "SELECT id FROM statuses WHERE workspace_id=?1 AND slug=?2",
        params![ws, slug],
        |r| r.get::<_, i64>(0),
    ) {
        conn.execute(
            "UPDATE items SET status_id=?1 WHERE id=?2",
            params![st, item_id],
        )?;
    }
    ledger::append(
        conn,
        ws,
        uid,
        Some(item_id),
        "update",
        None,
        Some(&status),
        None,
        Some(
            s(input, "comment")
                .unwrap_or_else(|| format!("Решение по неисправности: {status}"))
                .as_str(),
        ),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    Ok(json!({"ok": true, "id": id, "status": status}))
}

pub(crate) fn request_change(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let item_id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let ws = require_item_access(conn, uid, item_id)?;
    require_can_in_workspace(conn, uid, ws, "requestChanges")?;
    let payload = input.get("payload").cloned().unwrap_or(json!({}));
    conn.execute(
        "INSERT INTO change_requests (item_id, workspace_id, author_id, payload, comment, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![item_id, ws, uid, payload.to_string(), s(input, "comment"), now()],
    )?;
    let rid = conn.last_insert_rowid();
    notify_admins(
        conn,
        ws,
        item_id,
        "Заявка на правку",
        s(input, "comment")
            .as_deref()
            .unwrap_or("Изменение карточки"),
    );
    Ok(json!({"id": rid, "status": "pending"}))
}

/// Человекочитаемое имя записи справочника. Для пользователей это ФИО,
/// для остальных таблиц — колонка name.
pub(crate) fn lookup_name(conn: &Connection, table: &str, id: Option<i64>) -> Option<String> {
    let id = id?;
    let column = if table == "users" {
        "full_name"
    } else {
        "name"
    };
    let sql = format!("SELECT {column} FROM {table} WHERE id=?1");
    conn.query_row(&sql, params![id], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
}

/// Поля карточки, которые может менять заявка: ключ, подпись и справочник,
/// через который идентификатор разворачивается в название.
pub(crate) const CHANGEABLE_FIELDS: [(&str, &str, Option<&str>); 13] = [
    ("title", "Наименование", None),
    ("categoryId", "Категория", Some("categories")),
    ("brandId", "Бренд", Some("brands")),
    ("statusId", "Статус", Some("statuses")),
    ("responsibleUserId", "Ответственный", Some("users")),
    ("buildingSiteId", "Объект", Some("building_sites")),
    ("storageId", "Место хранения", Some("storages")),
    ("serialNumber", "Серийный номер", None),
    ("cost", "Стоимость", None),
    ("comment", "Комментарий", None),
    ("qrCode", "QR-код", None),
    ("calibratedUntil", "Поверка до", None),
    ("minQuantity", "Мин. остаток", None),
];

/// Приводит значение поля к строке для показа администратору.
pub(crate) fn display_value(
    conn: &Connection,
    raw: &Value,
    dictionary: Option<&str>,
) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    if let Some(table) = dictionary {
        let id = raw.as_i64().or_else(|| raw.as_f64().map(|v| v as i64));
        return lookup_name(conn, table, id);
    }
    match raw {
        Value::String(v) if v.is_empty() => None,
        Value::String(v) => Some(v.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "да".into() } else { "нет".into() }),
        _ => Some(raw.to_string()),
    }
}

/// Сравнение «было / предлагается» (ТЗ §4): администратор должен видеть
/// разницу, а не сырой JSON заявки.
pub(crate) fn describe_change(conn: &Connection, item_id: i64, payload: &Value) -> Value {
    let Some(before) = jsn::item_json(conn, item_id, false) else {
        return Value::Array(vec![]);
    };
    let mut rows = Vec::new();
    for (key, label, dictionary) in CHANGEABLE_FIELDS {
        let Some(proposed) = payload.get(key) else {
            continue;
        };
        let after = display_value(conn, proposed, dictionary);
        let before_raw = before.get(key).cloned().unwrap_or(Value::Null);
        let before_text = display_value(conn, &before_raw, dictionary);
        if before_text == after {
            continue; // поле в заявке есть, но значение то же — не шумим
        }
        rows.push(json!({
            "field": key,
            "label": label,
            "before": before_text,
            "after": after,
        }));
    }
    Value::Array(rows)
}

pub(crate) fn list_changes(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt = conn.prepare("SELECT id, item_id, workspace_id, author_id, payload, comment, status, reason, decided_by, created_at, decided_at FROM change_requests WHERE workspace_id=?1 ORDER BY id DESC LIMIT 200")?;
    let rows: Vec<Value> = stmt.query_map(params![ws], |r| {
        let author: i64 = r.get(3)?;
        let payload: String = r.get(4)?;
        Ok(json!({
            "id": r.get::<_, i64>(0)?, "itemId": r.get::<_, i64>(1)?, "workspaceId": r.get::<_, i64>(2)?,
            "authorId": author, "payload": serde_json::from_str::<Value>(&payload).unwrap_or(json!({})),
            "comment": r.get::<_, Option<String>>(5)?, "status": r.get::<_, String>(6)?,
            "reason": r.get::<_, Option<String>>(7)?, "decidedBy": r.get::<_, Option<i64>>(8)?,
            "createdAt": r.get::<_, String>(9)?, "decidedAt": r.get::<_, Option<String>>(10)?,
            "author": jsn::user_public(conn, author),
            "item": jsn::item_json(conn, r.get(1)?, false),
            "changes": describe_change(conn, r.get(1)?, &serde_json::from_str::<Value>(&payload).unwrap_or(json!({})))
        }))
    })?.filter_map(|x| x.ok()).collect();
    Ok(Value::Array(rows))
}

pub(crate) fn decide_change(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| decide_change_atomic(conn, input, user_id))
}

pub(crate) fn decide_change_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    let (item_id, ws, payload, request_comment): (i64, i64, String, Option<String>) = conn
        .query_row(
            "SELECT item_id, workspace_id, payload, comment FROM change_requests WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("Заявка не найдена"))?;
    require_member(conn, uid, ws)?;
    require_can_in_workspace(conn, uid, ws, "editItems")?;
    let already: String = conn.query_row(
        "SELECT status FROM change_requests WHERE id=?1",
        params![id],
        |r| r.get(0),
    )?;
    if already != "pending" {
        return Err(ApiError::conflict("Решение по заявке уже принято"));
    }
    let accept = b(input, "accept").unwrap_or(false);
    let status = if accept { "accepted" } else { "rejected" };
    if accept {
        // Правку применяем ДО отметки «принято»: если она не проходит проверки
        // (например, смена статуса без причины), заявка остаётся в работе,
        // а не «принятой», но не применённой.
        let mut patch = serde_json::from_str::<Value>(&payload)
            .map_err(|_| ApiError::bad("Заявка содержит некорректные данные"))?;
        if let Value::Object(ref mut o) = patch {
            o.insert("id".into(), json!(item_id));
            if !o.contains_key("reason") {
                let reason = s(input, "reason")
                    .or(request_comment)
                    .unwrap_or_else(|| "Принята заявка на правку".into());
                o.insert("reason".into(), json!(reason));
            }
        }
        items_update_atomic(conn, &patch, Some(uid))?;
    }
    conn.execute(
        "UPDATE change_requests SET status=?1, reason=?2, decided_by=?3, decided_at=?4 WHERE id=?5 AND status='pending'",
        params![status, s(input, "reason"), uid, now(), id],
    )?;
    Ok(json!({"ok": true, "id": id, "itemId": item_id, "status": status}))
}
