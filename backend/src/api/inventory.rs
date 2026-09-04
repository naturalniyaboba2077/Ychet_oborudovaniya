//! Инвентаризация и корректировки по её итогам.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn inv_sessions(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt = conn.prepare("SELECT id, number, workspace_id, status, started_by, created_at, completed_at FROM inventory_sessions WHERE workspace_id=?1 ORDER BY id DESC")?;
    let rows: Vec<Value> = stmt
        .query_map(params![ws], |r| {
            let id: i64 = r.get(0)?;
            let total: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM inventory_results WHERE session_id=?1",
                    params![id],
                    |x| x.get(0),
                )
                .unwrap_or(0);
            let checked: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM inventory_results WHERE session_id=?1 AND checked=1",
                    params![id],
                    |x| x.get(0),
                )
                .unwrap_or(0);
            Ok(json!({
                "id": id, "number": r.get::<_, String>(1)?, "workspaceId": r.get::<_, i64>(2)?,
                "status": r.get::<_, String>(3)?, "startedBy": r.get::<_, i64>(4)?,
                "createdAt": r.get::<_, String>(5)?, "completedAt": r.get::<_, Option<String>>(6)?,
                "totalItems": total, "checkedItems": checked,
                "starter": jsn::user_public(conn, r.get(4)?),
            }))
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(rows))
}

pub(crate) fn inv_session_full(conn: &Connection, id: i64) -> Option<Value> {
    conn.query_row(
        "SELECT id, number, workspace_id, status, started_by, created_at, completed_at FROM inventory_sessions WHERE id=?1",
        params![id],
        |r| {
            let mut results = Vec::new();
            let mut stmt = conn.prepare("SELECT id, session_id, item_id, expected_qty, actual_qty, checked FROM inventory_results WHERE session_id=?1").unwrap();
            for row in stmt.query_map(params![id], |x| {
                let item_id: i64 = x.get(2)?;
                Ok(json!({
                    "id": x.get::<_, i64>(0)?, "sessionId": x.get::<_, i64>(1)?, "itemId": item_id,
                    "expectedQty": x.get::<_, Option<f64>>(3)?, "actualQty": x.get::<_, Option<f64>>(4)?,
                    "checked": x.get::<_, i64>(5)? != 0,
                    "item": jsn::item_json(conn, item_id, false)
                }))
            }).unwrap().flatten() { results.push(row); }
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "number": r.get::<_, String>(1)?, "workspaceId": r.get::<_, i64>(2)?,
                "status": r.get::<_, String>(3)?, "startedBy": r.get::<_, i64>(4)?,
                "createdAt": r.get::<_, String>(5)?, "completedAt": r.get::<_, Option<String>>(6)?,
                "starter": jsn::user_public(conn, r.get(4)?),
                "results": results
            }))
        },
    ).ok()
}

pub(crate) fn inv_by_id(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    let ws: i64 = conn.query_row(
        "SELECT workspace_id FROM inventory_sessions WHERE id=?1",
        params![id],
        |r| r.get(0),
    )?;
    require_member(conn, uid, ws)?;
    inv_session_full(conn, id).ok_or_else(|| ApiError::not_found("Сессия не найдена"))
}

pub(crate) fn inv_results(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let sid = i64v(input, "sessionId").ok_or_else(|| ApiError::bad("sessionId"))?;
    let ws: i64 = conn.query_row(
        "SELECT workspace_id FROM inventory_sessions WHERE id=?1",
        params![sid],
        |r| r.get(0),
    )?;
    require_member(conn, uid, ws)?;
    let s = inv_session_full(conn, sid).ok_or_else(|| ApiError::not_found("Сессия не найдена"))?;
    Ok(s.get("results").cloned().unwrap_or(json!([])))
}

pub(crate) fn inv_create(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "inventory")?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM inventory_sessions WHERE workspace_id=?1",
        params![ws],
        |r| r.get(0),
    )?;
    let number = format!("ИНВ-{:03}", n + 1);
    conn.execute("INSERT INTO inventory_sessions (number, workspace_id, started_by, created_at) VALUES (?1,?2,?3,?4)", params![number, ws, uid, now()])?;
    let sid = conn.last_insert_rowid();
    let mut sql =
        String::from("SELECT id, quantity, quantitative FROM items WHERE workspace_id=?1");
    if let Some(st) = i64v(input, "storageId") {
        sql.push_str(&format!(" AND storage_id={st}"));
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(i64, Option<f64>, i64)> = stmt
        .query_map(params![ws], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|x| x.ok())
        .collect();
    for (id, qty, qnt) in rows {
        let exp = if qnt != 0 { qty.unwrap_or(0.0) } else { 1.0 };
        conn.execute("INSERT INTO inventory_results (session_id, item_id, expected_qty, checked) VALUES (?1,?2,?3,0)", params![sid, id, exp])?;
    }
    inv_session_full(conn, sid).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn inv_check(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "inventory")?;
    let sid = i64v(input, "sessionId").ok_or_else(|| ApiError::bad("sessionId"))?;
    let iid = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let (ws, status): (i64, String) = conn.query_row(
        "SELECT workspace_id,status FROM inventory_sessions WHERE id=?1",
        params![sid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    require_member(conn, uid, ws)?;
    if status != "in_progress" {
        return Err(ApiError::conflict("Инвентаризация уже завершена"));
    }
    require_item_access(conn, uid, iid)?;
    let checked = b(input, "checked").unwrap_or(true);
    if checked {
        conn.execute("UPDATE inventory_results SET checked=1, actual_qty=COALESCE(?3, actual_qty) WHERE session_id=?1 AND item_id=?2",
            params![sid, iid, f64v(input,"actualQty")])?;
    } else {
        conn.execute(
            "UPDATE inventory_results SET checked=0 WHERE session_id=?1 AND item_id=?2",
            params![sid, iid],
        )?;
    }
    inv_session_full(conn, sid).ok_or_else(|| ApiError::not_found("нет"))
}

/// Расхождения инвентаризации не затирают историю: каждое оформляется
/// отдельной корректирующей записью журнала, а для количественных позиций
/// остаток приводится к фактическому (ТЗ §4, «Инвентаризация»).
pub(crate) fn apply_inventory_corrections(
    conn: &Connection,
    session_id: i64,
    ws: i64,
    uid: i64,
    number: &str,
) -> Result<usize, ApiError> {
    let mut stmt = conn.prepare(
        "SELECT r.item_id, r.expected_qty, r.actual_qty, i.quantitative, i.title, i.internal_id
         FROM inventory_results r JOIN items i ON i.id = r.item_id
         WHERE r.session_id = ?1 AND r.checked = 1 AND r.actual_qty IS NOT NULL",
    )?;
    let rows: Vec<(i64, f64, f64, i64, String, String)> = stmt
        .query_map(params![session_id], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .filter_map(|x| x.ok())
        .collect();
    drop(stmt);

    let mut corrections = 0usize;
    for (item_id, expected, actual, quantitative, title, internal_id) in rows {
        let delta = actual - expected;
        if delta.abs() < 1e-9 {
            continue;
        }
        if quantitative != 0 {
            conn.execute(
                "UPDATE items SET quantity=?1 WHERE id=?2",
                params![actual, item_id],
            )?;
        }
        ledger::append(
            conn,
            ws,
            uid,
            Some(item_id),
            "inventory",
            None,
            None,
            Some(delta),
            Some(&format!(
                "Корректировка по {number}: {internal_id} {title}, учтено {expected}, фактически {actual}"
            )),
        )
        .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
        corrections += 1;
    }
    Ok(corrections)
}

pub(crate) fn inv_complete(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| inv_complete_atomic(conn, input, user_id))
}

pub(crate) fn inv_complete_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "inventory")?;
    let sid = i64v(input, "sessionId").ok_or_else(|| ApiError::bad("sessionId"))?;
    let ws: i64 = conn.query_row(
        "SELECT workspace_id FROM inventory_sessions WHERE id=?1",
        params![sid],
        |r| r.get(0),
    )?;
    require_member(conn, uid, ws)?;
    let number: String = conn.query_row(
        "SELECT number FROM inventory_sessions WHERE id=?1",
        params![sid],
        |r| r.get(0),
    )?;
    let changed = conn.execute(
        "UPDATE inventory_sessions SET status='completed', completed_at=?1 WHERE id=?2 AND status='in_progress'",
        params![now(), sid],
    )?;
    if changed != 1 {
        return Err(ApiError::conflict("Инвентаризация уже завершена"));
    }
    let corrections = apply_inventory_corrections(conn, sid, ws, uid, &number)?;
    ledger::append(
        conn,
        ws,
        uid,
        None,
        "inventory",
        None,
        None,
        None,
        Some(&format!(
            "Инвентаризация {number} завершена. Расхождений: {corrections}"
        )),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    let mut session = inv_session_full(conn, sid).ok_or_else(|| ApiError::not_found("нет"))?;
    session["corrections"] = json!(corrections);
    Ok(session)
}
