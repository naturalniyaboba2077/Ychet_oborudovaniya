//! Выдача, возврат и передача между сотрудниками.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn transfers_list(conn: &Connection, user_id: Option<i64>, outgoing: bool) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let sql = if outgoing {
        "SELECT id FROM transfers WHERE from_user_id=?1 AND status IN ('draft','pending') ORDER BY id DESC"
    } else {
        "SELECT id FROM transfers WHERE to_user_id=?1 AND status='pending' ORDER BY id DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let ids: Vec<i64> = stmt
        .query_map(params![uid], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Value::Array(
        ids.into_iter()
            .filter_map(|id| jsn::transfer_json(conn, id))
            .collect(),
    ))
}

pub(crate) fn transfer_by_id(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    let transfer =
        jsn::transfer_json(conn, id).ok_or_else(|| ApiError::not_found("Передача не найдена"))?;
    let ws = transfer["workspaceId"]
        .as_i64()
        .ok_or_else(|| ApiError::bad("Некорректная передача"))?;
    require_member(conn, uid, ws)?;
    let party =
        transfer["fromUserId"].as_i64() == Some(uid) || transfer["toUserId"].as_i64() == Some(uid);
    if !party && !user_can(conn, uid, "manageUsers") {
        return Err(ApiError::new(
            "FORBIDDEN",
            403,
            "Передача доступна только её участникам",
        ));
    }
    Ok(transfer)
}

/// Следующий номер передачи вида «ПП-0007».
///
/// Счётчик хранится в `kv`, а не выводится из таблицы: и COUNT(*), и MAX
/// проседают, когда передачи удаляют — освободившийся номер достаётся
/// следующей записи, и в отчётах оказываются две разные выдачи под одним
/// номером. Дополнительно берём максимум уже существующих: обмен мог
/// принести с другого узла номер больше нашего счётчика.
///
/// Оговорка: два узла, работающие офлайн,независимо выдадут одинаковый
/// номер — общего счётчика у них нет. После обмена коллизия видна, и
/// следующие номера её обходят.
pub(crate) fn next_transfer_code(conn: &Connection, ws: i64) -> String {
    let key = format!("transfer_seq:{ws}");
    let stored: i64 = sync::kv_get(conn, &key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let seen: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(CAST(substr(code, 4) AS INTEGER)), 0)
             FROM transfers WHERE workspace_id=?1 AND code LIKE 'ПП-%'",
            params![ws],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let mut n = stored.max(seen) + 1;
    for _ in 0..1000 {
        let code = format!("ПП-{n:04}");
        let busy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transfers WHERE workspace_id=?1 AND code=?2",
                params![ws, code],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if busy == 0 {
            sync::kv_set(conn, &key, &n.to_string());
            return code;
        }
        n += 1;
    }
    // Тысяча занятых подряд в норме невозможна, но молча выдать дубль хуже,
    // чем некрасивый, зато точно уникальный номер.
    format!("ПП-{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn checkout_policy(conn: &Connection, uid: i64) -> Value {
    jsn::user_public(conn, uid)
        .and_then(|u| u.get("checkoutPolicy").cloned())
        .unwrap_or_else(db::default_checkout_policy)
}

/// Списанный, отправленный в ремонт или на проверку предмет не участвует
/// в обороте — ни выдача, ни передача другому сотруднику.
pub(crate) fn ensure_item_circulates(
    conn: &Connection,
    item: &Value,
    item_id: i64,
) -> Result<(), ApiError> {
    match item["status"]["slug"].as_str() {
        Some("written-off") => {
            return Err(ApiError::bad("Списанный инструмент недоступен для выдачи"))
        }
        Some("in-repair") | Some("needs-check") => {
            return Err(ApiError::bad(
                "Инструмент на проверке или в ремонте, выдача запрещена",
            ))
        }
        _ => {}
    }
    let open_faults: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM faults WHERE item_id=?1 AND status IN ('open','repair')",
            params![item_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if open_faults > 0 {
        return Err(ApiError::bad(
            "По предмету есть неисправность, выдача запрещена",
        ));
    }
    Ok(())
}

pub(crate) fn take_one(
    conn: &mut Connection,
    uid: i64,
    item_id: i64,
    comment: Option<&str>,
    due_at: Option<&str>,
    photo_url: Option<&str>,
    qty: Option<f64>,
) -> ApiResult {
    atomic(conn, |conn| {
        take_one_atomic(conn, uid, item_id, comment, due_at, photo_url, qty)
    })
}

pub(crate) fn take_one_atomic(
    conn: &Connection,
    uid: i64,
    item_id: i64,
    comment: Option<&str>,
    due_at: Option<&str>,
    photo_url: Option<&str>,
    qty: Option<f64>,
) -> ApiResult {
    let item_ws = require_item_access(conn, uid, item_id)?;
    require_can_in_workspace(conn, uid, item_ws, "transferItems")?;
    let item = jsn::item_json(conn, item_id, false)
        .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
    ensure_item_circulates(conn, &item, item_id)?;
    // Штучный предмет всегда у кого-то одного. Забрать его «через голову»
    // держателя нельзя: на этот случай в ТЗ есть передача с подтверждением,
    // иначе факт изъятия нигде не всплывёт.
    if !item["quantitative"].as_bool().unwrap_or(false) {
        match item["responsibleUserId"].as_i64() {
            Some(holder) if holder == uid => {
                return Err(ApiError::bad("Инструмент уже у вас"));
            }
            Some(_) => {
                return Err(ApiError::bad(
                    "Инструмент числится за другим сотрудником — запросите передачу",
                ));
            }
            None => {}
        }
    }
    let policy = checkout_policy(conn, uid);
    if let Some(cats) = policy.get("allowedCategoryIds").and_then(|v| v.as_array()) {
        if !cats.is_empty() {
            let cat = item["categoryId"].as_i64();
            let ok = cat
                .map(|c| cats.iter().any(|x| x.as_i64() == Some(c)))
                .unwrap_or(false);
            if !ok {
                return Err(ApiError::bad("Вам не разрешено брать эту категорию"));
            }
        }
    }
    let allow_none = policy
        .get("allowNoDueDate")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if due_at.is_none() && !allow_none {
        return Err(ApiError::bad("Укажите срок возврата"));
    }
    if let (Some(due), Some(max_h)) = (due_at, policy.get("maxHours").and_then(|v| v.as_f64())) {
        if let Ok(due_ts) = chrono::DateTime::parse_from_rfc3339(due) {
            let hours =
                (due_ts.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_hours() as f64;
            if hours > max_h + 0.1 {
                return Err(ApiError::bad(format!(
                    "Срок больше разрешённого ({max_h} ч)"
                )));
            }
        }
    }
    let ws = item["workspaceId"].as_i64().unwrap_or(1);
    let from = item["responsibleUserId"]
        .as_i64()
        .or_else(|| item["storage"]["responsibleUserId"].as_i64())
        .unwrap_or(uid);
    let need_admin = policy
        .get("requireApproval")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let code = next_transfer_code(conn, ws);
    if item["quantitative"].as_bool().unwrap_or(false) {
        let take_qty = qty.unwrap_or(1.0);
        if take_qty <= 0.0 {
            return Err(ApiError::bad("Укажите количество"));
        }
        let stock = item["quantity"].as_f64().unwrap_or(0.0);
        if take_qty > stock + 1e-9 {
            return Err(ApiError::bad(format!("На складе только {stock}")));
        }
        if need_admin {
            conn.execute(
                "INSERT INTO transfers (code, item_id, from_user_id, to_user_id, to_storage_id, building_site_id, workspace_id, quantity, status, comment, no_confirmation, needs_admin, photo_url, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9,0,1,?10,?11)",
                params![code, item_id, from, uid, item["storageId"].as_i64(), item["buildingSiteId"].as_i64(), ws, take_qty, comment, photo_url, now()],
            )?;
            return Ok(
                json!({"pending": true, "code": code, "itemId": item_id, "quantity": take_qty, "message": "Заявка отправлена администратору"}),
            );
        }
        let changed = conn.execute(
            "UPDATE items SET quantity=quantity-?1 WHERE id=?2 AND quantity>=?1",
            params![take_qty, item_id],
        )?;
        if changed != 1 {
            return Err(ApiError::conflict("Остаток изменился; повторите операцию"));
        }
        conn.execute(
            "INSERT INTO item_holdings (item_id, user_id, quantity, due_at, comment, photo_url, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![item_id, uid, take_qty, due_at, comment, photo_url, now()],
        )?;
        conn.execute(
            "INSERT INTO transfers (code, item_id, from_user_id, to_user_id, to_storage_id, building_site_id, workspace_id, quantity, status, comment, no_confirmation, photo_url, created_at, completed_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'accepted',?9,1,?10,?11,?11)",
            params![code, item_id, from, uid, item["storageId"].as_i64(), item["buildingSiteId"].as_i64(), ws, take_qty, comment, photo_url, now()],
        )?;
        let title = item["title"].as_str().unwrap_or("");
        let to_name = jsn::user_public(conn, uid)
            .and_then(|u| u["fullName"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        ledger::append(
            conn,
            ws,
            uid,
            Some(item_id),
            "transfer_receive",
            Some("Склад"),
            Some(&to_name),
            Some(take_qty),
            Some(&format!("Выдача {code}: {take_qty} × {title}")),
        )
        .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
        return jsn::item_json(conn, item_id, false).ok_or_else(|| ApiError::bad("ошибка"));
    }
    if need_admin {
        conn.execute(
            "INSERT INTO transfers (code, item_id, from_user_id, to_user_id, to_storage_id, building_site_id, workspace_id, status, comment, no_confirmation, needs_admin, photo_url, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'pending',?8,0,1,?9,?10)",
            params![code, item_id, from, uid, item["storageId"].as_i64(), item["buildingSiteId"].as_i64(), ws, comment, photo_url, now()],
        )?;
        notify_admins(
            conn,
            ws,
            item_id,
            "Заявка на выдачу",
            &format!(
                "{} просит {} ({})",
                jsn::user_public(conn, uid)
                    .and_then(|u| u["fullName"].as_str().map(|s| s.to_string()))
                    .unwrap_or_default(),
                item["title"].as_str().unwrap_or(""),
                code
            ),
        );
        return Ok(
            json!({"pending": true, "code": code, "itemId": item_id, "message": "Заявка отправлена администратору"}),
        );
    }
    let in_work: Option<i64> = conn
        .query_row(
            "SELECT id FROM statuses WHERE workspace_id=?1 AND slug='in-work'",
            params![ws],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    // Условие в самом UPDATE — единственное, что защищает от одновременной
    // выдачи: проверка выше читала состояние до записи, и между ними предмет
    // мог уйти другому. Ноль изменённых строк означает, что опоздали.
    let taken = conn.execute(
        "UPDATE items SET responsible_user_id=?1, status_id=COALESCE(?2,status_id), due_at=?4
         WHERE id=?3 AND responsible_user_id IS NULL",
        params![uid, in_work, item_id, due_at],
    )?;
    if taken != 1 {
        return Err(ApiError::conflict(
            "Инструмент только что забрали; обновите список",
        ));
    }
    conn.execute(
        "INSERT INTO transfers (code, item_id, from_user_id, to_user_id, to_storage_id, building_site_id, workspace_id, status, comment, no_confirmation, photo_url, created_at, completed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,'accepted',?8,1,?9,?10,?10)",
        params![code, item_id, from, uid, item["storageId"].as_i64(), item["buildingSiteId"].as_i64(), ws, comment, photo_url, now()],
    )?;
    let title = item["title"].as_str().unwrap_or("");
    let from_name = jsn::user_public(conn, from)
        .and_then(|u| u["fullName"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "Склад".into());
    let to_name = jsn::user_public(conn, uid)
        .and_then(|u| u["fullName"].as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    ledger::append(
        conn,
        ws,
        from,
        Some(item_id),
        "transfer_send",
        Some(&from_name),
        Some(&to_name),
        None,
        Some(&format!("Выдача {code}: {title}")),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    ledger::append(
        conn,
        ws,
        uid,
        Some(item_id),
        "transfer_receive",
        Some(&from_name),
        Some(&to_name),
        None,
        Some(&format!("Получение {code}")),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    jsn::item_json(conn, item_id, false).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn transfers_take(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let qty = f64v(input, "quantity");
    let item = jsn::item_json(conn, id, false)
        .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
    let want = qty.unwrap_or(1.0).max(1.0);
    if !item["quantitative"].as_bool().unwrap_or(false) && want > 1.0 {
        let mut taken = Vec::new();
        let mut failed = Vec::new();
        match take_one(
            conn,
            uid,
            id,
            s(input, "comment").as_deref(),
            s(input, "dueAt").as_deref(),
            s(input, "photoUrl").as_deref(),
            None,
        ) {
            Ok(_) => taken.push(id),
            Err(e) => failed.push(json!({"itemId": id, "message": e.message})),
        }
        if let Some(members) = item
            .get("family")
            .and_then(|f| f.get("members"))
            .and_then(|v| v.as_array())
        {
            for m in members {
                if taken.len() as f64 >= want {
                    break;
                }
                let sid = m.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                if sid == id || m.get("inStock").and_then(|v| v.as_bool()) != Some(true) {
                    continue;
                }
                match take_one(
                    conn,
                    uid,
                    sid,
                    s(input, "comment").as_deref(),
                    s(input, "dueAt").as_deref(),
                    s(input, "photoUrl").as_deref(),
                    None,
                ) {
                    Ok(_) => taken.push(sid),
                    Err(e) => failed.push(json!({"itemId": sid, "message": e.message})),
                }
            }
        }
        return Ok(
            json!({"takenCount": taken.len(), "taken": taken, "failed": failed, "itemId": id}),
        );
    }
    take_one(
        conn,
        uid,
        id,
        s(input, "comment").as_deref(),
        s(input, "dueAt").as_deref(),
        s(input, "photoUrl").as_deref(),
        qty,
    )
}

pub(crate) fn transfers_take_many(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let ids = g(input, "itemIds").as_array().cloned().unwrap_or_default();
    let mut taken = Vec::new();
    let mut failed = Vec::new();
    for v in ids {
        let id = v.as_i64().unwrap_or(0);
        match take_one(
            conn,
            uid,
            id,
            s(input, "comment").as_deref(),
            s(input, "dueAt").as_deref(),
            s(input, "photoUrl").as_deref(),
            f64v(input, "quantity"),
        ) {
            Ok(_) => taken.push(id),
            Err(e) => failed.push(json!({"itemId": id, "message": e.message})),
        }
    }
    Ok(json!({"takenCount": taken.len(), "taken": taken, "failed": failed}))
}

pub(crate) fn transfers_return(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| transfers_return_atomic(conn, input, user_id))
}

pub(crate) fn transfers_return_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let item = jsn::item_json(conn, id, false)
        .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
    if item["quantitative"].as_bool().unwrap_or(false) {
        let held: f64 = conn.query_row(
            "SELECT COALESCE(SUM(quantity),0) FROM item_holdings WHERE item_id=?1 AND user_id=?2 AND returned_at IS NULL",
            params![id, uid], |r| r.get(0),
        ).unwrap_or(0.0);
        if held <= 0.0 {
            return Err(ApiError::bad("У вас нет этого материала"));
        }
        let give = f64v(input, "quantity").unwrap_or(held).min(held);
        conn.execute(
            "UPDATE item_holdings SET returned_at=?1 WHERE item_id=?2 AND user_id=?3 AND returned_at IS NULL",
            params![now(), id, uid],
        )?;
        if give + 1e-9 < held {
            conn.execute(
                "INSERT INTO item_holdings (item_id, user_id, quantity, created_at) VALUES (?1,?2,?3,?4)",
                params![id, uid, held - give, now()],
            )?;
        }
        conn.execute(
            "UPDATE items SET quantity=COALESCE(quantity,0)+?1 WHERE id=?2",
            params![give, id],
        )?;
        let vn = item["internalId"].as_str().unwrap_or("");
        let name = jsn::user_public(conn, uid)
            .and_then(|u| u["fullName"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        ledger::append(
            conn,
            item["workspaceId"].as_i64().unwrap_or(1),
            uid,
            Some(id),
            "transfer_send",
            Some(&name),
            Some("Склад"),
            Some(give),
            Some(&format!("Возврат {give} × {vn}")),
        )
        .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
        return jsn::item_json(conn, id, false).ok_or_else(|| ApiError::bad("ошибка"));
    }
    if item["responsibleUserId"].as_i64() != Some(uid) {
        if item["responsibleUserId"].is_null() {
            return Err(ApiError::bad("Инструмент уже на складе"));
        }
        return Err(ApiError::bad("Инструмент на другом сотруднике"));
    }
    let ws = item["workspaceId"].as_i64().unwrap_or(1);
    let in_stock: Option<i64> = conn
        .query_row(
            "SELECT id FROM statuses WHERE workspace_id=?1 AND slug='in-stock'",
            params![ws],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    conn.execute("UPDATE items SET responsible_user_id=NULL, building_site_id=NULL, status_id=COALESCE(?1,status_id), due_at=NULL WHERE id=?2", params![in_stock, id])?;
    let vn = item["internalId"].as_str().unwrap_or("");
    let name = jsn::user_public(conn, uid)
        .and_then(|u| u["fullName"].as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    ledger::append(
        conn,
        ws,
        uid,
        Some(id),
        "transfer_send",
        Some(&name),
        Some("Склад"),
        None,
        Some(&format!("Возврат {vn} на склад")),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    jsn::item_json(conn, id, false).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn transfers_prepare(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| transfers_prepare_atomic(conn, input, user_id))
}

pub(crate) fn transfers_prepare_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let item_id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    let to = i64v(input, "toUserId").ok_or_else(|| ApiError::bad("toUserId"))?;
    let item = jsn::item_json(conn, item_id, false)
        .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
    let ws = item["workspaceId"].as_i64().unwrap_or(1);
    require_member(conn, uid, ws)?;
    require_can_in_workspace(conn, uid, ws, "transferItems")?;
    require_member(conn, to, ws)
        .map_err(|_| ApiError::bad("Получатель не состоит в этом рабочем пространстве"))?;
    ensure_item_circulates(conn, &item, item_id)?;
    if !item["quantitative"].as_bool().unwrap_or(false)
        && item["responsibleUserId"].as_i64().is_some()
        && item["responsibleUserId"].as_i64() != Some(uid)
    {
        return Err(ApiError::new(
            "FORBIDDEN",
            403,
            "Передать инструмент может только ответственный сотрудник",
        ));
    }
    let status = if b(input, "asDraft").unwrap_or(false) {
        "draft"
    } else {
        "pending"
    };
    let code = next_transfer_code(conn, ws);
    conn.execute(
        "INSERT INTO transfers (code, item_id, from_user_id, to_user_id, to_storage_id, building_site_id, workspace_id, quantity, status, comment, no_confirmation, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![code, item_id, uid, to, i64v(input,"toStorageId"), i64v(input,"buildingSiteId"), ws, f64v(input,"quantity"), status, s(input,"comment"), b(input,"noConfirmation").unwrap_or(false) as i64, now()],
    )?;
    let tid = conn.last_insert_rowid();
    ledger::append(
        conn,
        ws,
        uid,
        Some(item_id),
        "transfer_send",
        None,
        None,
        None,
        Some(&format!("Передача {code} оформлена")),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    if to != uid {
        let title = item["title"].as_str().unwrap_or("");
        let from_name = jsn::user_public(conn, uid)
            .and_then(|u| u["fullName"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        conn.execute(
            "INSERT INTO notifications (user_id, item_id, type, title, text, created_at) VALUES (?1,?2,'transfer','Ожидает приёма',?3,?4)",
            params![to, item_id, format!("Передача {code}: {title} от {from_name}"), now()],
        )?;
    }
    jsn::transfer_json(conn, tid).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn transfers_accept(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
    accept: bool,
) -> ApiResult {
    atomic(conn, |conn| {
        transfers_accept_atomic(conn, input, user_id, accept)
    })
}

pub(crate) fn transfers_accept_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
    accept: bool,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    let t =
        jsn::transfer_json(conn, id).ok_or_else(|| ApiError::not_found("Передача не найдена"))?;
    let ws = t["workspaceId"]
        .as_i64()
        .ok_or_else(|| ApiError::bad("Некорректная передача"))?;
    require_member(conn, uid, ws)?;
    let needs_admin: bool = conn.query_row(
        "SELECT needs_admin != 0 FROM transfers WHERE id=?1",
        params![id],
        |row| row.get(0),
    )?;
    if needs_admin {
        require_can(conn, uid, "manageUsers")?;
    } else if t["toUserId"].as_i64() != Some(uid) {
        return Err(ApiError::new(
            "FORBIDDEN",
            403,
            "Подтвердить передачу может только получатель",
        ));
    }
    let st = t["status"].as_str().unwrap_or("");
    if st != "pending" && st != "draft" {
        return Err(ApiError::bad("Передача уже завершена"));
    }
    let new_st = if accept { "accepted" } else { "rejected" };
    conn.execute(
        "UPDATE transfers SET status=?1, completed_at=?2, comment=COALESCE(?3,comment) WHERE id=?4",
        params![new_st, now(), s(input, "comment"), id],
    )?;
    if accept {
        let item_id = t["itemId"]
            .as_i64()
            .ok_or_else(|| ApiError::bad("В передаче нет инструмента"))?;
        let item = jsn::item_json(conn, item_id, false)
            .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
        if item["quantitative"].as_bool().unwrap_or(false) {
            let quantity = t["quantity"]
                .as_f64()
                .filter(|q| *q > 0.0)
                .ok_or_else(|| ApiError::bad("В передаче не указано количество"))?;
            let changed = conn.execute(
                "UPDATE items SET quantity=quantity-?1 WHERE id=?2 AND quantity>=?1",
                params![quantity, item_id],
            )?;
            if changed != 1 {
                return Err(ApiError::conflict("Недостаточное количество на складе"));
            }
            conn.execute(
                "INSERT INTO item_holdings (item_id, user_id, quantity, created_at) VALUES (?1,?2,?3,?4)",
                params![item_id, t["toUserId"].as_i64(), quantity, now()],
            )?;
        } else {
            conn.execute("UPDATE items SET responsible_user_id=?1, storage_id=COALESCE(?2,storage_id), building_site_id=COALESCE(?3,building_site_id) WHERE id=?4",
                params![t["toUserId"].as_i64(), t["toStorageId"].as_i64(), t["buildingSiteId"].as_i64(), item_id])?;
        }
    }
    ledger::append(
        conn,
        ws,
        uid,
        t["itemId"].as_i64(),
        "transfer_receive",
        None,
        None,
        t["quantity"].as_f64(),
        Some(if accept {
            "Принята"
        } else {
            "Отклонена"
        }),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    jsn::transfer_json(conn, id).ok_or_else(|| ApiError::bad("ошибка"))
}

pub(crate) fn transfers_accept_all(conn: &mut Connection, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let mut stmt =
        conn.prepare("SELECT id FROM transfers WHERE to_user_id=?1 AND status='pending'")?;
    let ids: Vec<i64> = stmt
        .query_map(params![uid], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    drop(stmt);
    let mut accepted = Vec::new();
    for id in ids {
        if let Ok(v) = transfers_accept(conn, &json!({"id": id}), Some(uid), true) {
            accepted.push(v);
        }
    }
    Ok(json!({"acceptedCount": accepted.len(), "accepted": accepted}))
}
