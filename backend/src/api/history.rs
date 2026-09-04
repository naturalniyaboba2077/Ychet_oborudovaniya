//! Журнал операций: списание, пополнение, перемещение.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn history_list(conn: &Connection, input: &Value, types: &[&str]) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut sql = String::from("SELECT id FROM history_entries WHERE workspace_id=?1");
    if !types.is_empty() {
        sql.push_str(" AND type IN (");
        sql.push_str(
            &types
                .iter()
                .map(|t| format!("'{t}'"))
                .collect::<Vec<_>>()
                .join(","),
        );
        sql.push(')');
    }
    if let Some(id) = i64v(input, "itemId") {
        sql.push_str(&format!(" AND item_id={id}"));
    }
    sql.push_str(" ORDER BY id DESC LIMIT 500");
    let mut stmt = conn.prepare(&sql)?;
    let ids: Vec<i64> = stmt
        .query_map(params![ws], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    let mut out = Vec::new();
    for id in ids {
        if let Ok(v) = conn.query_row(
            "SELECT id, workspace_id, item_id, type, actor_user_id, from_label, to_label, quantity_delta, comment, hash, created_at, photo_url FROM history_entries WHERE id=?1",
            params![id],
            |r| {
                let actor: i64 = r.get(4)?;
                let item_id: Option<i64> = r.get(2)?;
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "workspaceId": r.get::<_, i64>(1)?,
                    "itemId": item_id,
                    "type": r.get::<_, String>(3)?,
                    "actorUserId": actor,
                    "fromLabel": r.get::<_, Option<String>>(5)?,
                    "toLabel": r.get::<_, Option<String>>(6)?,
                    "quantityDelta": r.get::<_, Option<f64>>(7)?,
                    "comment": r.get::<_, Option<String>>(8)?,
                    "opId": r.get::<_, String>(9)?,
                    "createdAt": r.get::<_, String>(10)?,
                    "photoUrl": r.get::<_, Option<String>>(11)?,
                    "actor": jsn::user_public(conn, actor),
                    "item": item_id.and_then(|i| jsn::item_json(conn, i, false)),
                }))
            },
        ) { out.push(v); }
    }
    Ok(Value::Array(out))
}

/// Требует ли группа фото при списании.
pub(crate) fn requires_writeoff_photo(conn: &Connection, ws: i64) -> bool {
    conn.query_row(
        "SELECT require_writeoff_photo FROM workspaces WHERE id=?1",
        params![ws],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or(0)
        != 0
}

/// Привязывает фото к уже созданной записи журнала. Отдельным шагом, чтобы
/// не расширять и без того длинную сигнатуру `ledger::append`.
pub(crate) fn attach_photo(
    conn: &Connection,
    entry: &Value,
    photo: Option<&str>,
) -> Result<(), ApiError> {
    let (Some(url), Some(id)) = (photo, entry.get("id").and_then(Value::as_i64)) else {
        return Ok(());
    };
    conn.execute(
        "UPDATE history_entries SET photo_url=?1 WHERE id=?2",
        params![url, id],
    )?;
    Ok(())
}

pub(crate) fn history_write_off(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| history_write_off_atomic(conn, input, user_id))
}

pub(crate) fn history_write_off_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "writeOff")?;
    if s(input, "comment").is_none() {
        return Err(ApiError::bad("Укажите причину списания"));
    }
    let id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let item_ws = require_item_access(conn, uid, id)?;
    // ТЗ §8: если группа так настроена, списание без фото не принимается.
    let photo = s(input, "photoUrl");
    if requires_writeoff_photo(conn, item_ws) && photo.is_none() {
        return Err(ApiError::bad(
            "В этой группе списание требует фото-подтверждения",
        ));
    }
    let item = jsn::item_json(conn, id, false).ok_or_else(|| ApiError::not_found("нет"))?;
    let ws = item["workspaceId"].as_i64().unwrap_or(1);
    if item["quantitative"].as_bool().unwrap_or(false) {
        let qty = f64v(input, "quantity").unwrap_or(1.0);
        if qty <= 0.0 {
            return Err(ApiError::bad("Количество должно быть больше нуля"));
        }
        let stock = item["quantity"].as_f64().unwrap_or(0.0);
        if qty > stock + 1e-9 {
            return Err(ApiError::bad(format!(
                "Нельзя списать {qty}: доступно {stock}"
            )));
        }
        let changed = conn.execute(
            "UPDATE items SET quantity=quantity-?1 WHERE id=?2 AND quantity>=?1",
            params![qty, id],
        )?;
        if changed != 1 {
            return Err(ApiError::conflict("Остаток изменился; повторите операцию"));
        }
        let entry = ledger::append(
            conn,
            ws,
            uid,
            Some(id),
            "write_off",
            None,
            None,
            Some(-qty),
            s(input, "comment").as_deref(),
        )
        .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
        attach_photo(conn, &entry, photo.as_deref())?;
    } else {
        let st = conn
            .query_row(
                "SELECT id FROM statuses WHERE workspace_id=?1 AND slug='written-off'",
                params![ws],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| ApiError::bad("В рабочем пространстве нет статуса списания"))?;
        conn.execute("UPDATE items SET status_id=?1 WHERE id=?2", params![st, id])?;
        let entry = ledger::append(
            conn,
            ws,
            uid,
            Some(id),
            "write_off",
            None,
            None,
            None,
            s(input, "comment").as_deref(),
        )
        .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
        attach_photo(conn, &entry, photo.as_deref())?;
    }
    jsn::item_json(conn, id, false).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn history_replenish(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| history_replenish_atomic(conn, input, user_id))
}

pub(crate) fn history_replenish_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "replenish")?;
    let id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let qty = f64v(input, "quantity").ok_or_else(|| ApiError::bad("quantity"))?;
    if qty <= 0.0 {
        return Err(ApiError::bad("Количество должно быть больше нуля"));
    }
    require_item_access(conn, uid, id)?;
    let item = jsn::item_json(conn, id, false).ok_or_else(|| ApiError::not_found("нет"))?;
    if !item["quantitative"].as_bool().unwrap_or(false) {
        return Err(ApiError::bad("Инструмент не количественный"));
    }
    conn.execute(
        "UPDATE items SET quantity=COALESCE(quantity,0)+?1 WHERE id=?2",
        params![qty, id],
    )?;
    ledger::append(
        conn,
        item["workspaceId"].as_i64().unwrap_or(1),
        uid,
        Some(id),
        "replenish",
        None,
        None,
        Some(qty),
        s(input, "comment").as_deref(),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    jsn::item_json(conn, id, false).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn history_move(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| history_move_atomic(conn, input, user_id))
}

pub(crate) fn history_move_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "editItems")?;
    let id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    require_item_access(conn, uid, id)?;
    conn.execute(
        "UPDATE items SET storage_id=COALESCE(?1,storage_id), building_site_id=?2 WHERE id=?3",
        params![
            i64v(input, "toStorageId"),
            i64v(input, "toBuildingSiteId"),
            id
        ],
    )?;
    let item = jsn::item_json(conn, id, false).ok_or_else(|| ApiError::not_found("нет"))?;
    ledger::append(
        conn,
        item["workspaceId"].as_i64().unwrap_or(1),
        uid,
        Some(id),
        "move",
        None,
        None,
        None,
        s(input, "comment").as_deref(),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    Ok(item)
}
