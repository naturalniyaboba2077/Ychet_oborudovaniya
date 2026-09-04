//! Отчёты по сотрудникам и по всему парку.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn reports_by_users(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt =
        conn.prepare("SELECT DISTINCT responsible_user_id FROM items WHERE workspace_id=?1")?;
    let uids: Vec<Option<i64>> = stmt
        .query_map(params![ws], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    let mut out = Vec::new();
    for uid in uids {
        let mut st = conn.prepare("SELECT id FROM items WHERE workspace_id=?1 AND ((?2 IS NULL AND responsible_user_id IS NULL) OR responsible_user_id=?2)")?;
        let ids: Vec<i64> = st
            .query_map(params![ws, uid], |r| r.get(0))?
            .filter_map(|x| x.ok())
            .collect();
        let items: Vec<Value> = ids
            .iter()
            .filter_map(|id| jsn::item_json(conn, *id, false))
            .collect();
        let total: f64 = items
            .iter()
            .map(|i| i["cost"].as_f64().unwrap_or(0.0))
            .sum();
        out.push(json!({
            "userId": uid,
            "user": uid.and_then(|i| jsn::user_public(conn, i)),
            "itemsCount": items.len(),
            "totalCost": total,
            "items": items
        }));
    }
    Ok(Value::Array(out))
}

pub(crate) fn reports_all(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let mut stmt =
        conn.prepare("SELECT id FROM items WHERE workspace_id=?1 ORDER BY created_at DESC")?;
    let ids: Vec<i64> = stmt
        .query_map(params![ws], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(
        ids.into_iter()
            .filter_map(|id| item_for_list(conn, id))
            .collect(),
    ))
}
