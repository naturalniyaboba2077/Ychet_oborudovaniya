//! Каталог: карточки предметов, фотографии, комментарии.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn workspaces_list(conn: &Connection) -> ApiResult {
    let ids: Vec<i64> = CURRENT_UID.with(|c| {
        if let Some(uid) = c.get() {
            conn.prepare("SELECT workspace_id FROM user_workspaces WHERE user_id=?1 ORDER BY id")
                .ok()
                .and_then(|mut stmt| {
                    stmt.query_map(params![uid], |r| r.get(0))
                        .ok()
                        .map(|rows| rows.filter_map(|x| x.ok()).collect())
                })
                .unwrap_or_default()
        } else {
            conn.prepare("SELECT id FROM workspaces ORDER BY id")
                .ok()
                .and_then(|mut stmt| {
                    stmt.query_map([], |r| r.get(0))
                        .ok()
                        .map(|rows| rows.filter_map(|x| x.ok()).collect())
                })
                .unwrap_or_default()
        }
    });
    Ok(Value::Array(
        ids.into_iter()
            .filter_map(|id| jsn::workspace_json(conn, id))
            .collect(),
    ))
}

pub(crate) fn transfer_counts(conn: &Connection, uid: i64) -> ApiResult {
    let outgoing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transfers WHERE from_user_id=?1 AND status IN ('draft','pending')",
        params![uid],
        |r| r.get(0),
    )?;
    let incoming: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transfers WHERE to_user_id=?1 AND status='pending'",
        params![uid],
        |r| r.get(0),
    )?;
    Ok(json!({"outgoing": outgoing, "incoming": incoming}))
}

/// Карточка для списка: без оригиналов снимков.
pub(crate) fn item_for_list(conn: &Connection, id: i64) -> Option<Value> {
    let mut item = jsn::item_json(conn, id, false)?;
    jsn::strip_full_photos(&mut item);
    Some(item)
}

pub(crate) fn items_list(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let page = i64v(input, "page").unwrap_or(1).max(1);
    let limit = i64v(input, "limit").unwrap_or(20).clamp(1, 500);
    let search = s(input, "search").map(|q| q.to_lowercase());
    let only_mine = b(input, "onlyMine").unwrap_or(false);
    let mut stmt = conn.prepare("SELECT id, title, internal_id, serial_number, responsible_user_id FROM items WHERE workspace_id=?1 ORDER BY created_at DESC, id DESC")?;
    let mut ids: Vec<i64> = Vec::new();
    let rows = stmt.query_map(params![ws], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;
    for row in rows.flatten() {
        let (id, title, internal, serial, resp) = row;
        if only_mine && resp != user_id {
            continue;
        }
        if let Some(ref q) = search {
            let blob = format!(
                "{} {} {}",
                title,
                internal,
                serial.clone().unwrap_or_default()
            )
            .to_lowercase();
            if !blob.contains(q) {
                continue;
            }
        }
        ids.push(id);
    }
    let total = ids.len() as i64;
    let start = ((page - 1) * limit) as usize;
    let has_more = total > start as i64 + limit;
    let rows: Vec<Value> = ids
        .into_iter()
        .skip(start)
        .take(limit as usize)
        .filter_map(|id| item_for_list(conn, id))
        .collect();
    Ok(json!({"rows": rows, "page": page, "limit": limit, "hasMore": has_more, "total": total}))
}

pub(crate) fn items_by_id(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    require_item_access(conn, uid, id)?;
    jsn::item_json(conn, id, true).ok_or_else(|| ApiError::not_found("Инструмент не найден"))
}

pub(crate) fn items_by_code(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let code = s(input, "code").ok_or_else(|| ApiError::bad("code"))?;
    let id: Option<i64> = conn.query_row(
        "SELECT id FROM items WHERE qr_code=?1 OR internal_id=?1 OR UPPER(qr_code)=UPPER(?1) OR UPPER(internal_id)=UPPER(?1) LIMIT 1",
        params![code], |r| r.get(0),
    ).optional().ok().flatten();
    let id = id.ok_or_else(|| ApiError::not_found("Инструмент с таким QR/номером не найден"))?;
    require_item_access(conn, uid, id)?;
    jsn::item_json(conn, id, false).ok_or_else(|| ApiError::not_found("Инструмент не найден"))
}

pub(crate) fn items_next_id(conn: &Connection, input: &Value) -> ApiResult {
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    let prefix: String = conn
        .query_row(
            "SELECT internal_id_prefix FROM workspaces WHERE id=?1",
            params![ws],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "ВН-".into());
    let mut stmt = conn.prepare("SELECT internal_id FROM items WHERE workspace_id=?1")?;
    let ids: Vec<String> = stmt
        .query_map(params![ws], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    let mut max = 0i64;
    for id in ids {
        if let Some(n) = id.strip_prefix(&prefix).and_then(|x| x.parse::<i64>().ok()) {
            if n > max {
                max = n;
            }
        }
    }
    Ok(json!(format!("{prefix}{:04}", max + 1)))
}

pub(crate) fn items_create(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| items_create_atomic(conn, input, user_id))
}

pub(crate) fn items_create_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let ws = i64v(input, "workspaceId").unwrap_or_else(|| ws_fallback(conn));
    require_member(conn, uid, ws)?;
    require_can_in_workspace(conn, uid, ws, "createItems")?;
    validate_item_references(conn, input, ws)?;
    let title = s(input, "title").ok_or_else(|| ApiError::bad("Название обязательно"))?;
    let internal = if let Some(v) = s(input, "internalId") {
        v
    } else {
        items_next_id(conn, &json!({"workspaceId": ws}))?
            .as_str()
            .unwrap_or("ВН-0001")
            .to_string()
    };
    let qr = s(input, "qrCode").or(Some(internal.clone()));
    conn.execute(
        "INSERT INTO items (internal_id, title, category_id, brand_id, status_id, responsible_user_id, building_site_id, storage_id, workspace_id, serial_number, cost, quantitative, quantity, unit, comment, qr_code, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            internal, title, i64v(input,"categoryId"), i64v(input,"brandId"), i64v(input,"statusId"),
            i64v(input,"responsibleUserId"), i64v(input,"buildingSiteId"), i64v(input,"storageId"), ws,
            s(input,"serialNumber"), f64v(input,"cost"), b(input,"quantitative").unwrap_or(false) as i64,
            f64v(input,"quantity"), s(input,"unit"), s(input,"comment"), qr, now()
        ],
    ).map_err(|e| ApiError::bad(e.to_string()))?;
    let id = conn.last_insert_rowid();
    if let Some(arr) = g(input, "photos").as_array() {
        for (i, p) in arr.iter().enumerate() {
            // Принимаем и строку с оригиналом, и пару {url, thumbUrl}.
            let (url, thumb) = match p {
                Value::String(url) => (Some(url.clone()), None),
                Value::Object(_) => (
                    p.get("url").and_then(Value::as_str).map(str::to_owned),
                    p.get("thumbUrl").and_then(Value::as_str).map(str::to_owned),
                ),
                _ => (None, None),
            };
            if let Some(url) = url {
                insert_photo(conn, id, &url, thumb.as_deref(), i == 0)?;
            }
        }
    }
    ledger::append(
        conn,
        ws,
        uid,
        Some(id),
        "create",
        None,
        Some(&title),
        None,
        Some("Инструмент добавлен в каталог"),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    jsn::item_json(conn, id, true).ok_or_else(|| ApiError::bad("не создан"))
}

pub(crate) fn items_update(
    conn: &mut Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    atomic(conn, |conn| items_update_atomic(conn, input, user_id))
}

pub(crate) fn items_update_atomic(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "editItems")?;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    require_item_access(conn, uid, id)?;
    let before = jsn::item_json(conn, id, false)
        .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
    let ws = before["workspaceId"]
        .as_i64()
        .ok_or_else(|| ApiError::bad("Некорректный item"))?;
    validate_item_references(conn, input, ws)?;
    let before_status = before["statusId"].as_i64();
    let next_status = if input.get("statusId").is_some() {
        i64v(input, "statusId")
    } else {
        before_status
    };
    let reason = status_change_reason(conn, input, before_status, next_status)?;
    conn.execute(
        "UPDATE items SET title=COALESCE(?2,title),
         category_id=CASE WHEN ?3 THEN ?4 ELSE category_id END,
         brand_id=CASE WHEN ?5 THEN ?6 ELSE brand_id END,
         status_id=CASE WHEN ?7 THEN ?8 ELSE status_id END,
         responsible_user_id=CASE WHEN ?9 THEN ?10 ELSE responsible_user_id END,
         building_site_id=CASE WHEN ?11 THEN ?12 ELSE building_site_id END,
         storage_id=CASE WHEN ?13 THEN ?14 ELSE storage_id END,
         serial_number=CASE WHEN ?15 THEN ?16 ELSE serial_number END,
         cost=CASE WHEN ?17 THEN ?18 ELSE cost END,
         comment=CASE WHEN ?19 THEN ?20 ELSE comment END,
         qr_code=CASE WHEN ?21 THEN ?22 ELSE qr_code END,
         calibrated_until=CASE WHEN ?23 THEN ?24 ELSE calibrated_until END,
         min_quantity=CASE WHEN ?25 THEN ?26 ELSE min_quantity END
         WHERE id=?1",
        params![
            id,
            s(input, "title"),
            input.get("categoryId").is_some(),
            i64v(input, "categoryId"),
            input.get("brandId").is_some(),
            i64v(input, "brandId"),
            input.get("statusId").is_some(),
            i64v(input, "statusId"),
            input.get("responsibleUserId").is_some(),
            i64v(input, "responsibleUserId"),
            input.get("buildingSiteId").is_some(),
            i64v(input, "buildingSiteId"),
            input.get("storageId").is_some(),
            i64v(input, "storageId"),
            input.get("serialNumber").is_some(),
            s(input, "serialNumber"),
            input.get("cost").is_some(),
            f64v(input, "cost"),
            input.get("comment").is_some(),
            s(input, "comment"),
            input.get("qrCode").is_some(),
            s(input, "qrCode"),
            input.get("calibratedUntil").is_some(),
            s(input, "calibratedUntil"),
            input.get("minQuantity").is_some(),
            f64v(input, "minQuantity")
        ],
    )?;
    // Смена статуса и места хранения не должна выглядеть как безымянная правка:
    // журнал фиксирует переход и причину.
    let mut note = String::from("Данные инструмента обновлены");
    if before_status != next_status {
        let (from_name, _) = status_label(conn, before_status);
        let (to_name, _) = status_label(conn, next_status);
        note = format!(
            "Статус: {} → {}",
            from_name.unwrap_or_else(|| "—".into()),
            to_name.unwrap_or_else(|| "—".into())
        );
        if let Some(text) = &reason {
            note.push_str(&format!(". Причина: {text}"));
        }
    }
    let before_storage = before["storageId"].as_i64();
    let next_storage = if input.get("storageId").is_some() {
        i64v(input, "storageId")
    } else {
        before_storage
    };
    if before_storage != next_storage {
        let name: Option<String> = next_storage.and_then(|sid| {
            conn.query_row("SELECT name FROM storages WHERE id=?1", params![sid], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .ok()
            .flatten()
        });
        note.push_str(&format!(
            ". Место хранения: {}",
            name.unwrap_or_else(|| "не указано".into())
        ));
    }
    ledger::append(
        conn,
        ws,
        uid,
        Some(id),
        "update",
        None,
        None,
        None,
        Some(&note),
    )
    .map_err(|e| ApiError::internal(format!("Ошибка журнала: {e}")))?;
    jsn::item_json(conn, id, true).ok_or_else(|| ApiError::not_found("нет"))
}

pub(crate) fn items_remove(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "deleteItems")?;
    let _ = uid;
    let id = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
    require_item_access(conn, uid, id)?;
    conn.execute("DELETE FROM item_photos WHERE item_id=?1", params![id])?;
    conn.execute("DELETE FROM items WHERE id=?1", params![id])?;
    Ok(json!({"ok": true}))
}

/// Контрольная сумма вложения (ТЗ §5): по ней видно подмену снимка.
pub(crate) fn photo_checksum(url: &str) -> String {
    hex::encode(Sha256::digest(url.as_bytes()))
}

pub(crate) fn insert_photo(
    conn: &Connection,
    item_id: i64,
    url: &str,
    thumb: Option<&str>,
    is_title: bool,
) -> Result<i64, ApiError> {
    conn.execute(
        "INSERT INTO item_photos (item_id, url, thumb_url, sha256, is_title) VALUES (?1,?2,?3,?4,?5)",
        params![
            item_id,
            url,
            thumb.unwrap_or(url),
            photo_checksum(url),
            is_title as i64
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn items_add_photo(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    require_can(conn, uid, "editItems")?;
    let item_id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    require_item_access(conn, uid, item_id)?;
    let url = s(input, "url").ok_or_else(|| ApiError::bad("url"))?;
    let is_title = b(input, "isTitle").unwrap_or(false);
    let thumb = s(input, "thumbUrl");
    let id = insert_photo(conn, item_id, &url, thumb.as_deref(), is_title)?;
    Ok(json!({
        "id": id,
        "itemId": item_id,
        "url": url,
        "thumbUrl": thumb.unwrap_or(url.clone()),
        "sha256": photo_checksum(&url),
        "isTitle": is_title
    }))
}

pub(crate) fn items_add_comment(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let item_id = i64v(input, "itemId").ok_or_else(|| ApiError::bad("itemId"))?;
    require_item_access(conn, uid, item_id)?;
    let text = s(input, "text").ok_or_else(|| ApiError::bad("text"))?;
    conn.execute(
        "INSERT INTO item_comments (item_id, user_id, text, created_at) VALUES (?1,?2,?3,?4)",
        params![item_id, uid, text, now()],
    )?;
    Ok(
        json!({"id": conn.last_insert_rowid(), "itemId": item_id, "userId": uid, "text": text, "user": jsn::user_public(conn, uid)}),
    )
}
