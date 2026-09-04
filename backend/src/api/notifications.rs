//! Уведомления: просрочка возврата, остатки, поверка.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn emit_overdue_and_stock(conn: &Connection) {
    let nows = now();
    if let Ok(mut stmt) = conn.prepare("SELECT id, workspace_id, title, responsible_user_id, due_at FROM items WHERE due_at IS NOT NULL AND responsible_user_id IS NOT NULL") {
        let rows: Vec<(i64, i64, String, i64, String)> = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        }).ok().map(|x| x.filter_map(|y| y.ok()).collect()).unwrap_or_default();
        for (id, _ws, title, resp, due) in rows {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&due) {
                if dt.with_timezone(&chrono::Utc) < chrono::Utc::now() {
                    let exists: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM notifications WHERE item_id=?1 AND type='reminder' AND text LIKE '%просроч%'",
                        params![id], |r| r.get(0),
                    ).unwrap_or(0);
                    if exists == 0 {
                        let _ = conn.execute(
                            "INSERT INTO notifications (user_id, item_id, type, title, text, created_at) VALUES (?1,?2,'reminder','Просроченный возврат',?3,?4)",
                            params![resp, id, format!("Просрочен возврат: {title}"), nows.clone()],
                        );
                    }
                }
            }
        }
    }
    if let Ok(mut stmt) = conn.prepare("SELECT id, workspace_id, title, quantity, min_quantity FROM items WHERE quantitative=1 AND min_quantity IS NOT NULL AND quantity IS NOT NULL AND quantity < min_quantity") {
        let rows: Vec<(i64, i64, String, f64, f64)> = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        }).ok().map(|x| x.filter_map(|y| y.ok()).collect()).unwrap_or_default();
        for (id, ws, title, qty, minq) in rows {
            let exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM notifications WHERE item_id=?1 AND title='Мало на складе' AND read=0",
                params![id], |r| r.get(0),
            ).unwrap_or(0);
            if exists == 0 {
                notify_admins(conn, ws, id, "Мало на складе", &format!("{title}: {qty} (мин. {minq})"));
            }
        }
    }
    if let Ok(mut stmt) = conn.prepare("SELECT id, workspace_id, title, calibrated_until FROM items WHERE calibrated_until IS NOT NULL") {
        let rows: Vec<(i64, i64, String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .ok().map(|x| x.filter_map(|y| y.ok()).collect()).unwrap_or_default();
        for (id, ws, title, until) in rows {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&until) {
                if dt.with_timezone(&chrono::Utc) < chrono::Utc::now() {
                    let exists: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM notifications WHERE item_id=?1 AND title='Истекла поверка' AND read=0",
                        params![id], |r| r.get(0),
                    ).unwrap_or(0);
                    if exists == 0 {
                        notify_admins(conn, ws, id, "Истекла поверка", &format!("{title}: срок поверки прошёл"));
                    }
                }
            }
        }
    }
}

pub(crate) fn notif_list(conn: &Connection, user_id: Option<i64>) -> ApiResult {
    emit_overdue_and_stock(conn);
    let uid = require_user(conn, user_id)?;
    let mut stmt = conn.prepare("SELECT id, user_id, item_id, type, title, text, read, created_at FROM notifications WHERE user_id=?1 ORDER BY id DESC LIMIT 100")?;
    let rows: Vec<Value> = stmt
        .query_map(params![uid], |r| {
            let item_id: Option<i64> = r.get(2)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "userId": r.get::<_, i64>(1)?, "itemId": item_id,
                "type": r.get::<_, String>(3)?, "title": r.get::<_, Option<String>>(4)?,
                "text": r.get::<_, String>(5)?, "read": r.get::<_, i64>(6)? != 0,
                "createdAt": r.get::<_, String>(7)?,
                "item": item_id.and_then(|i| jsn::item_json(conn, i, false))
            }))
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(rows))
}

pub(crate) fn notif_unread(conn: &Connection, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM notifications WHERE user_id=?1 AND read=0",
        params![uid],
        |r| r.get(0),
    )?;
    Ok(json!({"count": n}))
}

pub(crate) fn notif_mark(
    conn: &Connection,
    input: &Value,
    all: bool,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    if all {
        conn.execute(
            "UPDATE notifications SET read=1 WHERE user_id=?1 AND read=0",
            params![uid],
        )?;
    } else if let Some(id) = i64v(input, "id") {
        conn.execute(
            "UPDATE notifications SET read=1 WHERE id=?1 AND user_id=?2",
            params![id, uid],
        )?;
    }
    Ok(json!({"ok": true}))
}

pub(crate) fn notify_admins(conn: &Connection, ws: i64, item_id: i64, title: &str, text: &str) {
    let mut stmt = match conn.prepare("SELECT user_id FROM user_workspaces WHERE workspace_id=?1") {
        Ok(s) => s,
        Err(_) => return,
    };
    let ids: Vec<i64> = stmt
        .query_map(params![ws], |r| r.get(0))
        .ok()
        .map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();
    for uid in ids {
        if user_can(conn, uid, "manageUsers") || user_can(conn, uid, "editItems") {
            let _ = conn.execute(
                "INSERT INTO notifications (user_id, item_id, type, title, text, created_at) VALUES (?1,?2,'system',?3,?4,?5)",
                params![uid, item_id, title, text, now()],
            );
        }
    }
}
