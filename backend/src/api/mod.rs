mod admin;
mod auth;
mod chat;
mod faults;
mod history;
mod inventory;
mod items;
mod notifications;
mod profile;
mod reports;
mod transfers;

use crate::json as jsn;
use auth::*;
// Вызывается из обработчика /auth/google/callback в main.rs.
use crate::{db, ledger, sync};
use admin::*;
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
pub use auth::google_finish;
use chat::*;
use faults::*;
use history::*;
use inventory::*;
use items::*;
use notifications::*;
use profile::*;
use reports::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use transfers::*;
use uuid::Uuid;

thread_local! {
    static CURRENT_UID: Cell<Option<i64>> = const { Cell::new(None) };
}

fn ws_fallback(conn: &Connection) -> i64 {
    CURRENT_UID.with(|c| {
        if let Some(uid) = c.get() {
            if let Ok(id) = conn.query_row(
                "SELECT workspace_id FROM user_workspaces WHERE user_id=?1 ORDER BY id DESC LIMIT 1",
                params![uid],
                |r| r.get(0),
            ) {
                return id;
            }
        }
        jsn::default_ws(conn)
    })
}

#[derive(Debug)]
pub struct ApiError {
    pub message: String,
    pub code: &'static str,
    pub http: u16,
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("BAD_REQUEST", 400, e.to_string())
    }
}

impl ApiError {
    fn new(code: &'static str, http: u16, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code,
            http,
        }
    }
    fn unauth(m: impl Into<String>) -> Self {
        Self::new("UNAUTHORIZED", 401, m)
    }
    fn not_found(m: impl Into<String>) -> Self {
        Self::new("NOT_FOUND", 404, m)
    }
    fn bad(m: impl Into<String>) -> Self {
        Self::new("BAD_REQUEST", 400, m)
    }
    fn conflict(m: impl Into<String>) -> Self {
        Self::new("CONFLICT", 409, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new("INTERNAL_SERVER_ERROR", 500, m)
    }
}

type ApiResult = Result<Value, ApiError>;

pub fn is_mutation(procedure: &str) -> bool {
    !matches!(
        procedure,
        "ping"
            | "auth.directory"
            | "auth.options"
            | "auth.me"
            | "auth.inviteInfo"
            | "meta.currentUser"
            | "meta.transferCounts"
            | "meta.workspaces"
            | "items.list"
            | "items.byId"
            | "items.byCode"
            | "items.nextInternalId"
            | "items.faults"
            | "items.changeRequests"
            | "chat.list"
            | "sync.status"
            | "sync.peers"
            | "sync.conflicts"
            | "transfers.outgoing"
            | "transfers.incoming"
            | "transfers.byId"
            | "history.movements"
            | "history.quantityOps"
            | "history.all"
            | "inventory.sessions"
            | "inventory.byId"
            | "inventory.results"
            | "notifications.list"
            | "notifications.unreadCount"
            | "reports.byUsers"
            | "reports.quantityTransactions"
            | "reports.allItems"
            | "profile.get"
            | "admin.users.list"
            | "admin.users.defaultRights"
            | "admin.workspaces.list"
            | "admin.workspaces.invites"
            | "admin.storages.list"
            | "admin.buildingSites.list"
            | "admin.dictionaries.list"
    )
}

fn atomic<T>(
    conn: &mut Connection,
    operation: impl FnOnce(&Connection) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let tx = conn.transaction()?;
    let result = operation(&tx)?;
    tx.commit()?;
    Ok(result)
}

fn g<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}
fn s(input: &Value, key: &str) -> Option<String> {
    g(input, key)
        .as_str()
        .map(|x| x.to_string())
        .filter(|x| !x.is_empty())
}
fn i64v(input: &Value, key: &str) -> Option<i64> {
    g(input, key)
        .as_i64()
        .or_else(|| g(input, key).as_u64().map(|x| x as i64))
        .or_else(|| g(input, key).as_f64().map(|x| x as i64))
}
fn f64v(input: &Value, key: &str) -> Option<f64> {
    g(input, key)
        .as_f64()
        .or_else(|| g(input, key).as_i64().map(|x| x as f64))
}
fn b(input: &Value, key: &str) -> Option<bool> {
    g(input, key).as_bool()
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn hash_password(password: &str) -> String {
    let salt_raw = rand::random::<[u8; 16]>();
    let salt = SaltString::encode_b64(&salt_raw).expect("valid salt length");
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2 password hashing")
        .to_string()
}
fn verify_password(password: &str, stored: &str) -> bool {
    if stored.starts_with("$argon2") {
        return PasswordHash::new(stored).ok().is_some_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        });
    }
    let Some((salt, digest)) = stored.split_once('$') else {
        return false;
    };
    let mut h = Sha256::new();
    h.update(format!("{salt}:{password}"));
    hex::encode(h.finalize()) == digest
}

fn find_user_phone(conn: &Connection, phone: &str) -> Option<i64> {
    let want = db::digits_only(phone);
    let mut stmt = conn.prepare("SELECT id, phone FROM users").ok()?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .ok()?
        .filter_map(|x| x.ok())
        .collect();
    rows.into_iter()
        .find(|(_, p)| db::digits_only(p) == want)
        .map(|(id, _)| id)
}

fn require_user(conn: &Connection, user_id: Option<i64>) -> Result<i64, ApiError> {
    let Some(id) = user_id else {
        return Err(ApiError::unauth("Войдите в систему"));
    };
    let status: Option<String> = conn
        .query_row("SELECT status FROM users WHERE id=?1", params![id], |r| {
            r.get(0)
        })
        .optional()
        .ok()
        .flatten();
    match status.as_deref() {
        Some("disabled") => Err(ApiError::unauth("Аккаунт заблокирован")),
        Some(_) => Ok(id),
        None => Err(ApiError::unauth("Пользователь не найден")),
    }
}

fn user_can(conn: &Connection, uid: i64, key: &str) -> bool {
    let mut rights = db::default_rights();
    if let Some(stored) = jsn::user_public(conn, uid).and_then(|u| u.get("roleRights").cloned()) {
        if let (Value::Object(ref mut dest), Value::Object(src)) = (&mut rights, stored) {
            for (k, v) in src {
                dest.insert(k, v);
            }
        }
    }
    rights.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn require_can(conn: &Connection, uid: i64, key: &str) -> Result<(), ApiError> {
    if user_can(conn, uid, key) {
        Ok(())
    } else {
        Err(ApiError::new(
            "FORBIDDEN",
            403,
            "Недостаточно прав для этого действия",
        ))
    }
}

/// Права пользователя в пространстве, наложенные на набор по умолчанию.
///
/// Наложение обязательно: у записей, созданных до появления нового права,
/// ключа просто нет, и без слияния такой пользователь потерял бы доступ,
/// которого у него никто не отбирал.
fn merged_rights(conn: &Connection, uid: i64, ws: i64) -> Value {
    let raw: Option<String> = conn.query_row(
        "SELECT COALESCE(uw.rights_json,u.role_rights) FROM user_workspaces uw JOIN users u ON u.id=uw.user_id WHERE uw.user_id=?1 AND uw.workspace_id=?2",
        params![uid, ws], |r| r.get(0),
    ).optional().ok().flatten().flatten();
    let mut rights = db::default_rights();
    if let Some(stored) = raw.and_then(|v| serde_json::from_str::<Value>(&v).ok()) {
        if let (Value::Object(dest), Value::Object(src)) = (&mut rights, stored) {
            for (k, v) in src {
                dest.insert(k, v);
            }
        }
    }
    rights
}

fn require_can_in_workspace(
    conn: &Connection,
    uid: i64,
    ws: i64,
    key: &str,
) -> Result<(), ApiError> {
    let rights = merged_rights(conn, uid, ws);
    if rights.get(key).and_then(Value::as_bool).unwrap_or(false) {
        Ok(())
    } else {
        Err(ApiError::new(
            "FORBIDDEN",
            403,
            "Недостаточно прав в этом рабочем пространстве",
        ))
    }
}

/// Пространство пользователя по умолчанию — то же, которое подставит `ws_fallback`.
fn own_workspace(conn: &Connection, uid: i64) -> Option<i64> {
    conn.query_row(
        "SELECT workspace_id FROM user_workspaces WHERE user_id=?1 ORDER BY id DESC LIMIT 1",
        params![uid],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn require_member(conn: &Connection, uid: i64, workspace_id: i64) -> Result<(), ApiError> {
    let member: bool = conn
        .query_row(
            "SELECT 1 FROM user_workspaces WHERE user_id=?1 AND workspace_id=?2",
            params![uid, workspace_id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if member {
        Ok(())
    } else {
        Err(ApiError::new(
            "FORBIDDEN",
            403,
            "Нет доступа к рабочему пространству",
        ))
    }
}

fn require_item_access(conn: &Connection, uid: i64, item_id: i64) -> Result<i64, ApiError> {
    let ws = conn
        .query_row(
            "SELECT workspace_id FROM items WHERE id=?1",
            params![item_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("Инструмент не найден"))?;
    require_member(conn, uid, ws)?;
    Ok(ws)
}

fn validate_item_references(conn: &Connection, input: &Value, ws: i64) -> Result<(), ApiError> {
    for (key, table) in [
        ("categoryId", "categories"),
        ("brandId", "brands"),
        ("statusId", "statuses"),
        ("storageId", "storages"),
        ("buildingSiteId", "building_sites"),
    ] {
        if input.get(key).is_some() {
            if let Some(id) = i64v(input, key) {
                let sql = format!("SELECT COUNT(*) FROM {table} WHERE id=?1 AND workspace_id=?2");
                let found: i64 = conn.query_row(&sql, params![id, ws], |r| r.get(0))?;
                if found == 0 {
                    return Err(ApiError::bad(format!(
                        "{key} относится к другому рабочему пространству"
                    )));
                }
            }
        }
    }
    if input.get("responsibleUserId").is_some() {
        if let Some(user) = i64v(input, "responsibleUserId") {
            require_member(conn, user, ws)
                .map_err(|_| ApiError::bad("Ответственный не состоит в рабочем пространстве"))?;
        }
    }
    Ok(())
}

/// Статусы, перевод в которые требует явной причины (ТЗ §8).
const STATUSES_REQUIRING_REASON: [&str; 4] = ["in-repair", "needs-check", "written-off", "broken"];

fn status_label(conn: &Connection, status_id: Option<i64>) -> (Option<String>, Option<String>) {
    let Some(id) = status_id else {
        return (None, None);
    };
    conn.query_row(
        "SELECT name, slug FROM statuses WHERE id=?1",
        params![id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        },
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or((None, None))
}

/// Причина перевода в «неисправен», «на ремонте» или «списан».
/// Возвращает текст причины, если он обязателен и указан.
fn status_change_reason(
    conn: &Connection,
    input: &Value,
    before_status: Option<i64>,
    next_status: Option<i64>,
) -> Result<Option<String>, ApiError> {
    if before_status == next_status {
        return Ok(None);
    }
    let (name, slug) = status_label(conn, next_status);
    let slug = slug.unwrap_or_default();
    if !STATUSES_REQUIRING_REASON.contains(&slug.as_str()) {
        return Ok(None);
    }
    let reason = s(input, "reason").or_else(|| s(input, "comment"));
    match reason {
        Some(text) if text.trim().chars().count() >= 3 => Ok(Some(text)),
        _ => Err(ApiError::bad(format!(
            "Укажите причину перевода в статус «{}»",
            name.unwrap_or(slug)
        ))),
    }
}

fn required_admin_right(procedure: &str) -> Option<&'static str> {
    if procedure.starts_with("admin.users.") {
        Some("manageUsers")
    } else if procedure.starts_with("admin.workspaces.") {
        Some("manageWorkspaces")
    } else if procedure.starts_with("admin.storages.") {
        Some("manageStorages")
    } else if procedure.starts_with("admin.buildingSites.") {
        Some("manageSites")
    } else if procedure.starts_with("admin.dictionaries.") {
        Some("manageDictionaries")
    } else if procedure.starts_with("sync.") || procedure.starts_with("backup.") {
        Some("manageWorkspaces")
    } else {
        None
    }
}

fn required_right(procedure: &str) -> Option<&'static str> {
    if let Some(right) = required_admin_right(procedure) {
        return Some(right);
    }
    if matches!(procedure, "items.create") {
        Some("createItems")
    } else if matches!(procedure, "items.remove") {
        Some("deleteItems")
    } else if matches!(
        procedure,
        "items.update"
            | "items.addPhoto"
            | "history.move"
            | "items.resolveFault"
            | "items.decideChange"
    ) {
        Some("editItems")
    } else if matches!(procedure, "items.reportFault") {
        Some("reportFaults")
    } else if matches!(procedure, "items.requestChange") {
        Some("requestChanges")
    } else if procedure.starts_with("items.") {
        Some("viewItems")
    } else if matches!(
        procedure,
        "transfers.accept" | "transfers.reject" | "transfers.acceptAll"
    ) {
        Some("acceptTransfers")
    } else if procedure.starts_with("transfers.") {
        Some("transferItems")
    } else if matches!(procedure, "history.writeOff") {
        Some("writeOff")
    } else if matches!(procedure, "history.replenish") {
        Some("replenish")
    } else if procedure.starts_with("history.") {
        Some("viewHistory")
    } else if procedure.starts_with("inventory.") {
        Some("inventory")
    } else if procedure.starts_with("reports.") {
        Some("viewReports")
    } else {
        None
    }
}

fn target_workspace(
    conn: &Connection,
    procedure: &str,
    input: &Value,
) -> Result<Option<i64>, ApiError> {
    if let Some(ws) = i64v(input, "workspaceId") {
        return Ok(Some(ws));
    }
    if let Some(item_id) = i64v(input, "itemId") {
        return Ok(conn
            .query_row(
                "SELECT workspace_id FROM items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .optional()?);
    }
    let id = i64v(input, "id").or_else(|| i64v(input, "sessionId"));
    let Some(id) = id else { return Ok(None) };
    let table = if matches!(procedure, "items.resolveFault") {
        Some("faults")
    } else if matches!(procedure, "items.decideChange") {
        Some("change_requests")
    } else if procedure.starts_with("items.") {
        Some("items")
    } else if procedure.starts_with("transfers.") {
        Some("transfers")
    } else if procedure.starts_with("inventory.") {
        Some("inventory_sessions")
    } else if matches!(
        procedure,
        "admin.workspaces.update" | "admin.workspaces.remove"
    ) {
        return Ok(Some(id));
    } else if procedure.starts_with("admin.storages.") {
        Some("storages")
    } else if procedure.starts_with("admin.buildingSites.") {
        Some("building_sites")
    } else {
        None
    };
    if procedure.starts_with("admin.dictionaries.")
        && !matches!(
            procedure,
            "admin.dictionaries.list" | "admin.dictionaries.create"
        )
    {
        let kind = s(input, "kind").unwrap_or_else(|| "categories".into());
        let table = dict_table(&kind)?;
        let sql = format!("SELECT workspace_id FROM {table} WHERE id=?1");
        return Ok(conn.query_row(&sql, params![id], |r| r.get(0)).optional()?);
    }
    let Some(table) = table else { return Ok(None) };
    let sql = format!("SELECT workspace_id FROM {table} WHERE id=?1");
    Ok(conn.query_row(&sql, params![id], |r| r.get(0)).optional()?)
}

fn require_shared_workspace(
    conn: &Connection,
    actor: i64,
    target_user: i64,
) -> Result<(), ApiError> {
    let shared = conn.query_row(
        "SELECT 1 FROM user_workspaces a JOIN user_workspaces b ON b.workspace_id=a.workspace_id
         WHERE a.user_id=?1 AND b.user_id=?2 LIMIT 1",
        params![actor, target_user], |_| Ok(true),
    ).optional()?.unwrap_or(false);
    if shared {
        Ok(())
    } else {
        Err(ApiError::new(
            "FORBIDDEN",
            403,
            "Пользователь из другого рабочего пространства",
        ))
    }
}

pub fn dispatch(
    conn: &mut Connection,
    procedure: &str,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    CURRENT_UID.with(|c| c.set(user_id));
    let public = matches!(
        procedure,
        "ping"
            | "auth.login"
            | "auth.register"
            | "auth.joinRegister"
            | "auth.inviteInfo"
            | "auth.logout"
            | "auth.googleBegin"
            | "auth.options"
    ) || (procedure == "auth.directory"
        && std::env::var("MESHKEEPER_DEMO_LOGIN").as_deref() == Ok("1"));
    if !public {
        let uid = require_user(conn, user_id)?;
        let target_ws = target_workspace(conn, procedure, input)?;
        // Если пространство в запросе не указано, обработчик всё равно возьмёт
        // пространство пользователя по умолчанию — права проверяем там же,
        // иначе проверку можно было бы обойти, просто не передав workspaceId.
        let effective_ws = match target_ws {
            Some(ws) => {
                require_member(conn, uid, ws)?;
                Some(ws)
            }
            None => own_workspace(conn, uid),
        };
        if let Some(right) = required_right(procedure) {
            match effective_ws {
                Some(ws) => require_can_in_workspace(conn, uid, ws, right)?,
                None => require_can(conn, uid, right)?,
            }
        }
        if matches!(procedure, "admin.users.update" | "admin.users.remove") {
            let target = i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?;
            require_shared_workspace(conn, uid, target)?;
        }
    }
    // Обработчик выполняется ровно один раз: повторный вызов создавал бы
    // дубли на мутациях.
    let mut value = dispatch_inner(conn, procedure, input, user_id)?;
    // Фото и местонахождение — отдельные права (ТЗ §4). Прячем их в ответе,
    // а не в каждом обработчике: карточка предмета встречается вложенной
    // в историю, заявки, отчёты и передачи.
    if let Some(uid) = user_id {
        if let Some(ws) = target_workspace(conn, procedure, input)
            .ok()
            .flatten()
            .or_else(|| own_workspace(conn, uid))
        {
            let rights = merged_rights(conn, uid, ws);
            let hide_photos = !rights
                .get("viewPhotos")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let hide_location = !rights
                .get("viewLocation")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if hide_photos || hide_location {
                redact_item_fields(&mut value, hide_photos, hide_location);
            }
        }
    }
    Ok(value)
}

/// Рекурсивно вычищает из ответа поля карточки, закрытые правами.
fn redact_item_fields(value: &mut Value, hide_photos: bool, hide_location: bool) {
    match value {
        Value::Array(items) => {
            for item in items {
                redact_item_fields(item, hide_photos, hide_location);
            }
        }
        Value::Object(map) => {
            // Признак карточки предмета: у неё есть внутренний номер и название.
            let is_item = map.contains_key("internalId") && map.contains_key("title");
            if is_item {
                if hide_photos {
                    map.remove("photos");
                    map.remove("photoUrl");
                }
                if hide_location {
                    for key in ["storage", "storageId", "buildingSite", "buildingSiteId"] {
                        map.remove(key);
                    }
                }
            }
            for (_, nested) in map.iter_mut() {
                redact_item_fields(nested, hide_photos, hide_location);
            }
        }
        _ => {}
    }
}

fn dispatch_inner(
    conn: &mut Connection,
    procedure: &str,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    match procedure {
        "ping" => Ok(json!({"ok": true, "ts": chrono::Utc::now().timestamp_millis()})),
        "auth.directory" => {
            if std::env::var("MESHKEEPER_DEMO_LOGIN").as_deref() == Ok("1") {
                auth_directory(conn)
            } else {
                Err(ApiError::not_found("Процедура отключена"))
            }
        }
        "auth.options" => auth_options(conn),
        "auth.googleBegin" => auth_google_begin(conn, input, user_id),
        "auth.login" => auth_login(conn, input),
        "auth.register" => auth_register(conn, input),
        "auth.join" => auth_join(conn, input, user_id),
        "auth.joinRegister" => auth_join_register(conn, input),
        "auth.logout" => Ok(json!({"ok": true})),
        "auth.me" => {
            Ok(jsn::user_public(conn, require_user(conn, user_id)?).unwrap_or(Value::Null))
        }
        "auth.inviteInfo" => invite_info(conn, input),
        "meta.currentUser" => {
            Ok(jsn::user_public(conn, require_user(conn, user_id)?).unwrap_or(Value::Null))
        }
        "meta.transferCounts" => transfer_counts(conn, require_user(conn, user_id)?),
        "meta.workspaces" => workspaces_list(conn),
        "items.list" => items_list(conn, input, user_id),
        "items.byId" => items_by_id(conn, input, user_id),
        "items.byCode" => items_by_code(conn, input, user_id),
        "items.nextInternalId" => items_next_id(conn, input),
        "items.create" => items_create(conn, input, user_id),
        "items.update" => items_update(conn, input, user_id),
        "items.remove" => items_remove(conn, input, user_id),
        "items.addPhoto" => items_add_photo(conn, input, user_id),
        "items.addComment" => items_add_comment(conn, input, user_id),
        "items.reportFault" => report_fault(conn, input, user_id),
        "items.faults" => list_faults(conn, input),
        "items.resolveFault" => resolve_fault(conn, input, user_id),
        "items.requestChange" => request_change(conn, input, user_id),
        "items.changeRequests" => list_changes(conn, input),
        "items.decideChange" => decide_change(conn, input, user_id),
        "chat.list" => chat_list(conn, input, user_id),
        "chat.send" => chat_send(conn, input, user_id),
        "sync.status" => Ok(crate::sync::status(conn)),
        "sync.peers" => Ok(crate::sync::list_peers(conn)),
        "sync.addPeer" => {
            let url = s(input, "url").ok_or_else(|| ApiError::bad("Укажите адрес узла"))?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ApiError::bad(
                    "Адрес должен начинаться с http:// или https://",
                ));
            }
            Ok(crate::sync::add_peer(
                conn,
                &url,
                s(input, "name").as_deref(),
                None,
            ))
        }
        "sync.conflicts" => Ok(crate::sync::list_conflicts(conn)),
        "sync.resolveConflict" => {
            let uid = require_user(conn, user_id)?;
            require_can(conn, uid, "editItems")?;
            crate::sync::resolve_conflict(
                conn,
                i64v(input, "id").ok_or_else(|| ApiError::bad("id"))?,
                i64v(input, "responsibleUserId"),
                uid,
            )
            .map_err(|e| ApiError::bad(e.to_string()))
        }
        "sync.pullNow" => {
            if std::env::var("MESHKEEPER_UPSTREAM")
                .ok()
                .filter(|u| !u.trim().is_empty())
                .is_none()
            {
                return Err(ApiError::bad(
                    "Сервер не настроен: задайте MESHKEEPER_UPSTREAM на этом узле",
                ));
            }
            crate::sync::request_sync_now();
            Ok(json!({"ok": true, "queued": true}))
        }
        "backup.export" => backup_export(conn, input, user_id),
        "backup.import" => backup_import(conn, input, user_id),
        "transfers.outgoing" => transfers_list(conn, user_id, true),
        "transfers.incoming" => transfers_list(conn, user_id, false),
        "transfers.byId" => transfer_by_id(conn, input, user_id),
        "transfers.prepare" => transfers_prepare(conn, input, user_id),
        "transfers.accept" => transfers_accept(conn, input, user_id, true),
        "transfers.reject" => transfers_accept(conn, input, user_id, false),
        "transfers.acceptAll" => transfers_accept_all(conn, user_id),
        "transfers.take" => transfers_take(conn, input, user_id),
        "transfers.takeMany" => transfers_take_many(conn, input, user_id),
        "transfers.returnItem" => transfers_return(conn, input, user_id),
        "history.movements" => {
            history_list(conn, input, &["move", "transfer_send", "transfer_receive"])
        }
        "history.quantityOps" => history_list(conn, input, &["write_off", "replenish"]),
        "history.all" => history_list(conn, input, &[]),
        "history.writeOff" => history_write_off(conn, input, user_id),
        "history.replenish" => history_replenish(conn, input, user_id),
        "history.move" => history_move(conn, input, user_id),
        "inventory.sessions" => inv_sessions(conn, input),
        "inventory.byId" => inv_by_id(conn, input, user_id),
        "inventory.results" => inv_results(conn, input, user_id),
        "inventory.create" => inv_create(conn, input, user_id),
        "inventory.checkItem" => inv_check(conn, input, user_id),
        "inventory.complete" => inv_complete(conn, input, user_id),
        "notifications.list" => notif_list(conn, user_id),
        "notifications.unreadCount" => notif_unread(conn, user_id),
        "notifications.markRead" => notif_mark(conn, input, false, user_id),
        "notifications.markAllRead" => notif_mark(conn, input, true, user_id),
        "reports.byUsers" => reports_by_users(conn, input),
        "reports.quantityTransactions" => history_list(conn, input, &["write_off", "replenish"]),
        "reports.allItems" => reports_all(conn, input),
        "profile.get" => profile_get(conn, user_id),
        "profile.update" => profile_update(conn, input, user_id),
        "profile.changePassword" => profile_password(conn, input, user_id),
        "admin.users.list" => admin_users(conn, input),
        "admin.users.create" => admin_user_create(conn, input),
        "admin.users.update" => admin_user_update(conn, input, user_id),
        "admin.users.remove" => admin_user_remove(conn, input, user_id),
        "admin.users.invite" => admin_user_invite(conn, input, user_id),
        "admin.users.defaultRights" => Ok(db::default_rights()),
        "admin.workspaces.list" => workspaces_list(conn),
        "admin.workspaces.create" => ws_create(conn, input, user_id),
        "admin.workspaces.update" => ws_update(conn, input),
        "admin.workspaces.remove" => remove_workspace(conn, input),
        "admin.workspaces.createInvite" => ws_create_invite(conn, input, user_id),
        "admin.workspaces.invites" => ws_invites(conn, input),
        "admin.storages.list" => storages_list(conn, input),
        "admin.storages.create" => storage_create(conn, input),
        "admin.storages.update" => storage_update(conn, input),
        "admin.storages.remove" => {
            conn.execute(
                "DELETE FROM storages WHERE id=?1",
                params![i64v(input, "id").unwrap_or(0)],
            )?;
            Ok(json!({"ok": true}))
        }
        "admin.buildingSites.list" => sites_list(conn, input),
        "admin.buildingSites.create" => site_create(conn, input),
        "admin.buildingSites.update" => site_update(conn, input),
        "admin.buildingSites.remove" => {
            conn.execute(
                "DELETE FROM building_sites WHERE id=?1",
                params![i64v(input, "id").unwrap_or(0)],
            )?;
            Ok(json!({"ok": true}))
        }
        "admin.dictionaries.list" => dict_list(conn, input),
        "admin.dictionaries.create" => dict_create(conn, input),
        "admin.dictionaries.update" => dict_update(conn, input),
        "admin.dictionaries.remove" => dict_remove(conn, input),
        _ => Err(ApiError::not_found(format!("Нет процедуры {procedure}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_db() -> (Connection, PathBuf, [i64; 3], i64) {
        let path = std::env::temp_dir().join(format!("meshkeeper-api-{}.db", Uuid::new_v4()));
        let conn = db::open(&path).expect("test database");
        conn.execute(
            "INSERT INTO workspaces (name, timezone, internal_id_prefix, created_at) VALUES ('Test','UTC','T-',?1)",
            params![now()],
        )
        .unwrap();
        let ws = conn.last_insert_rowid();
        let mut users = [0; 3];
        for (index, slot) in users.iter_mut().enumerate() {
            let rights = if index == 0 {
                db::owner_rights()
            } else {
                db::default_rights()
            };
            conn.execute(
                "INSERT INTO users (full_name, phone, status, role_rights, created_at)
                 VALUES (?1,?2,'active',?3,?4)",
                params![
                    format!("User {index}"),
                    format!("+7000000000{index}"),
                    rights.to_string(),
                    now()
                ],
            )
            .unwrap();
            *slot = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO user_workspaces (user_id, workspace_id) VALUES (?1,?2)",
                params![*slot, ws],
            )
            .unwrap();
        }
        (conn, path, users, ws)
    }

    fn insert_item(
        conn: &Connection,
        ws: i64,
        responsible: Option<i64>,
        quantitative: bool,
        quantity: Option<f64>,
    ) -> i64 {
        conn.execute(
            "INSERT INTO items (internal_id, title, responsible_user_id, workspace_id, quantitative, quantity, comment, qr_code, created_at)
             VALUES (?1,'Test item',?2,?3,?4,?5,'keep','QR-KEEP',?6)",
            params![
                format!("T-{}", Uuid::new_v4()),
                responsible,
                ws,
                quantitative as i64,
                quantity,
                now()
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Делает запись в журнал невозможной, чтобы проверить откат мутации.
    fn break_ledger(conn: &Connection) {
        conn.execute_batch(
            "CREATE TRIGGER block_ledger BEFORE INSERT ON history_entries
             BEGIN SELECT RAISE(ABORT, 'ledger unavailable'); END;",
        )
        .unwrap();
    }

    fn cleanup(conn: Connection, path: PathBuf) {
        drop(conn);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn item_patch_preserves_absent_nullable_fields_and_clears_explicit_null() {
        let (mut conn, path, users, ws) = test_db();
        let item_id = insert_item(&conn, ws, Some(users[0]), false, None);

        items_update(
            &mut conn,
            &json!({"id": item_id, "title": "Renamed"}),
            Some(users[0]),
        )
        .unwrap();
        let preserved: (Option<i64>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT responsible_user_id, comment, qr_code FROM items WHERE id=?1",
                params![item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            preserved,
            (Some(users[0]), Some("keep".into()), Some("QR-KEEP".into()))
        );

        items_update(
            &mut conn,
            &json!({"id": item_id, "responsibleUserId": null, "comment": null}),
            Some(users[0]),
        )
        .unwrap();
        let cleared: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT responsible_user_id, comment FROM items WHERE id=?1",
                params![item_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cleared, (None, None));
        cleanup(conn, path);
    }

    #[test]
    fn quantity_writeoff_rejects_underflow_without_mutation_or_ledger_entry() {
        let (mut conn, path, users, ws) = test_db();
        let item_id = insert_item(&conn, ws, None, true, Some(5.0));
        let error = history_write_off(
            &mut conn,
            &json!({"itemId": item_id, "quantity": 6.0, "comment": "damage"}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(error.http, 400);
        let quantity: f64 = conn
            .query_row(
                "SELECT quantity FROM items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_entries WHERE item_id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(quantity, 5.0);
        assert_eq!(events, 0);
        cleanup(conn, path);
    }

    #[test]
    fn item_update_rolls_back_when_ledger_append_fails() {
        let (mut conn, path, users, ws) = test_db();
        let item_id = insert_item(&conn, ws, Some(users[0]), false, None);
        break_ledger(&conn);

        let error = items_update(
            &mut conn,
            &json!({"id": item_id, "title": "Must roll back"}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(error.http, 500);
        let title: String = conn
            .query_row(
                "SELECT title FROM items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Test item");
        cleanup(conn, path);
    }

    #[test]
    fn transfer_can_only_be_accepted_by_recipient() {
        let (mut conn, path, users, ws) = test_db();
        let item_id = insert_item(&conn, ws, Some(users[0]), false, None);
        conn.execute(
            "INSERT INTO transfers (code, item_id, from_user_id, to_user_id, workspace_id, status, no_confirmation, created_at)
             VALUES ('P-1',?1,?2,?3,?4,'pending',0,?5)",
            params![item_id, users[0], users[1], ws, now()],
        )
        .unwrap();
        let transfer_id = conn.last_insert_rowid();

        let error = transfers_accept(&mut conn, &json!({"id": transfer_id}), Some(users[0]), true)
            .unwrap_err();
        assert_eq!(error.http, 403);
        let status: String = conn
            .query_row(
                "SELECT status FROM transfers WHERE id=?1",
                params![transfer_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");

        transfers_accept(&mut conn, &json!({"id": transfer_id}), Some(users[1]), true).unwrap();
        let responsible: Option<i64> = conn
            .query_row(
                "SELECT responsible_user_id FROM items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(responsible, Some(users[1]));
        cleanup(conn, path);
    }

    #[test]
    fn inventory_completion_is_atomic_and_idempotent() {
        let (mut conn, path, users, ws) = test_db();
        conn.execute(
            "INSERT INTO inventory_sessions (number, workspace_id, status, started_by, created_at)
             VALUES ('INV-1',?1,'in_progress',?2,?3)",
            params![ws, users[0], now()],
        )
        .unwrap();
        let session_id = conn.last_insert_rowid();

        inv_complete(&mut conn, &json!({"sessionId": session_id}), Some(users[0])).unwrap();
        let error =
            inv_complete(&mut conn, &json!({"sessionId": session_id}), Some(users[0])).unwrap_err();
        assert_eq!(error.http, 409);
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_entries WHERE workspace_id=?1 AND type='inventory'",
                params![ws],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1);
        cleanup(conn, path);
    }

    #[test]
    fn id_only_item_route_rejects_cross_workspace_access() {
        let (mut conn, path, users, _ws) = test_db();
        conn.execute(
            "INSERT INTO workspaces (name, timezone, internal_id_prefix, created_at) VALUES ('Other','UTC','O-',?1)",
            params![now()],
        ).unwrap();
        let other_ws = conn.last_insert_rowid();
        let item = insert_item(&conn, other_ws, None, false, None);
        let error = dispatch(
            &mut conn,
            "items.byId",
            &json!({"id": item}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(error.http, 403);
        cleanup(conn, path);
    }

    #[test]
    fn passwordless_account_cannot_log_in() {
        let (conn, path, users, _ws) = test_db();
        let error = auth_login(
            &conn,
            &json!({"phone": "+70000000000", "password": "anything"}),
        )
        .unwrap_err();
        assert_eq!(error.http, 401);
        let hash: Option<String> = conn
            .query_row(
                "SELECT password_hash FROM users WHERE id=?1",
                params![users[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert!(hash.is_none());
        cleanup(conn, path);
    }

    #[test]
    fn expired_invite_is_rejected_everywhere() {
        let (mut conn, path, users, ws) = test_db();
        let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        conn.execute(
            "INSERT INTO invites (workspace_id, token, role, created_by, max_uses, expires_at, created_at)
             VALUES (?1,'expired-token','member',?2,20,?3,?4)",
            params![ws, users[0], past, now()],
        )
        .unwrap();

        let info = invite_info(&conn, &json!({"token": "expired-token"})).unwrap_err();
        assert_eq!(info.http, 400);

        let joined = dispatch(
            &mut conn,
            "auth.joinRegister",
            &json!({"token": "expired-token", "fullName": "Поздний", "phone": "+79995550000", "password": "LongEnoughPass1"}),
            None,
        )
        .unwrap_err();
        assert_eq!(joined.http, 400);

        let users_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(users_after, users.len() as i64);
        cleanup(conn, path);
    }

    #[test]
    fn fresh_invite_carries_expiry_and_role() {
        let (mut conn, path, users, ws) = test_db();
        let created = dispatch(
            &mut conn,
            "admin.workspaces.createInvite",
            &json!({"workspaceId": ws, "role": "viewer", "maxUses": 5}),
            Some(users[0]),
        )
        .unwrap();
        let expires = created["expiresAt"].as_str().expect("expiresAt");
        assert!(chrono::DateTime::parse_from_rfc3339(expires).unwrap() > chrono::Utc::now());
        assert_eq!(created["role"].as_str(), Some("viewer"));
        assert_eq!(created["payload"]["role"].as_str(), Some("viewer"));
        cleanup(conn, path);
    }

    /// Приглашение можно переслать кому угодно, поэтому предъявитель не
    /// должен получать чужую карточку, вписав чужой телефон.
    #[test]
    fn google_invite_cannot_take_over_an_existing_account() {
        let (mut conn, path, users, ws) = test_db();
        let owner_phone: String = conn
            .query_row(
                "SELECT phone FROM users WHERE id=?1",
                params![users[0]],
                |r| r.get(0),
            )
            .unwrap();
        let created = dispatch(
            &mut conn,
            "admin.workspaces.createInvite",
            &json!({"workspaceId": ws, "role": "member", "maxUses": 5}),
            Some(users[0]),
        )
        .unwrap();
        let token = created["token"].as_str().unwrap().to_string();

        let identity = crate::google::Identity {
            sub: "chuzhoy-google-akkaunt".into(),
            email: "chuzhoy@example.com".into(),
            name: "Чужой".into(),
        };
        let pending = crate::google::Pending {
            invite_token: Some(token),
            phone: Some(owner_phone),
            full_name: Some("Чужой".into()),
            link_user_id: None,
        };
        let attempt = google_finish(&conn, &identity, &pending);
        assert!(
            attempt.is_err(),
            "захват чужого аккаунта должен отклоняться"
        );

        // Владелец остался при своём: Google к нему не прицепился.
        let sub: Option<String> = conn
            .query_row(
                "SELECT google_sub FROM users WHERE id=?1",
                params![users[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sub, None);
        cleanup(conn, path);
    }

    #[test]
    fn google_invite_creates_a_passwordless_account() {
        let (mut conn, path, users, ws) = test_db();
        let created = dispatch(
            &mut conn,
            "admin.workspaces.createInvite",
            &json!({"workspaceId": ws, "role": "member", "maxUses": 5}),
            Some(users[0]),
        )
        .unwrap();
        let token = created["token"].as_str().unwrap().to_string();

        let identity = crate::google::Identity {
            sub: "novyy-google-akkaunt".into(),
            email: "montazhnik@example.com".into(),
            name: "Монтажник".into(),
        };
        let pending = crate::google::Pending {
            invite_token: Some(token),
            phone: Some("+79995552222".into()),
            full_name: Some("Монтажник".into()),
            link_user_id: None,
        };
        let uid = google_finish(&conn, &identity, &pending).expect("вход через Google");

        // Пароля у такого аккаунта нет вовсе — войти им по паролю нельзя.
        let hash: Option<String> = conn
            .query_row(
                "SELECT password_hash FROM users WHERE id=?1",
                params![uid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(hash.unwrap_or_default().is_empty());
        let member: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_workspaces WHERE user_id=?1 AND workspace_id=?2",
                params![uid, ws],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(member, 1, "приглашение должно завести членство");

        // Повторный вход тем же Google — тот же человек, не второй аккаунт.
        let again = google_finish(
            &conn,
            &identity,
            &crate::google::Pending {
                invite_token: None,
                phone: None,
                full_name: None,
                link_user_id: None,
            },
        )
        .expect("повторный вход");
        assert_eq!(again, uid);
        cleanup(conn, path);
    }

    #[test]
    fn google_without_invite_is_refused_for_unknown_person() {
        let (conn, path, _users, _ws) = test_db();
        let attempt = google_finish(
            &conn,
            &crate::google::Identity {
                sub: "nikto".into(),
                email: "nikto@example.com".into(),
                name: "Никто".into(),
            },
            &crate::google::Pending {
                invite_token: None,
                phone: Some("+79995553333".into()),
                full_name: Some("Никто".into()),
                link_user_id: None,
            },
        );
        assert!(attempt.is_err(), "свободной регистрации быть не должно");
        cleanup(conn, path);
    }

    /// Взять из рук коллеги нельзя: для этого есть передача с подтверждением.
    /// Без этой проверки любой участник тихо переписывал бы на себя предмет,
    /// за который отвечает другой, и в отчётах пропадал бы факт изъятия.
    #[test]
    fn taking_an_item_held_by_someone_else_is_refused() {
        let (mut conn, path, users, ws) = test_db();
        let item = insert_item(&conn, ws, None, false, None);

        dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": item}),
            Some(users[1]),
        )
        .expect("свободный предмет берётся");

        let grab = dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": item}),
            Some(users[2]),
        );
        assert!(grab.is_err(), "перехват чужого предмета должен отклоняться");

        // Предмет остался за первым, и лишней записи о выдаче не появилось.
        let holder: Option<i64> = conn
            .query_row(
                "SELECT responsible_user_id FROM items WHERE id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(holder, Some(users[1]));
        let transfers: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transfers WHERE item_id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            transfers, 1,
            "неудачная попытка не должна оставлять передачу"
        );
        cleanup(conn, path);
    }

    /// COUNT(*) освобождал номер вместе с удалённой передачей, и следующая
    /// выдача получала тот же «ПП-0001».
    #[test]
    fn transfer_codes_are_not_reused_after_deletion() {
        let (mut conn, path, users, ws) = test_db();
        let first = insert_item(&conn, ws, None, false, None);
        let second = insert_item(&conn, ws, None, false, None);
        dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": first}),
            Some(users[1]),
        )
        .unwrap();
        let code_one: String = conn
            .query_row(
                "SELECT code FROM transfers WHERE item_id=?1",
                params![first],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute("DELETE FROM transfers WHERE item_id=?1", params![first])
            .unwrap();
        dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": second}),
            Some(users[1]),
        )
        .unwrap();
        let code_two: String = conn
            .query_row(
                "SELECT code FROM transfers WHERE item_id=?1",
                params![second],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(
            code_one, code_two,
            "номер передачи не должен переиспользоваться"
        );
        cleanup(conn, path);
    }

    /// Голый DELETE оставлял предметы и историю с несуществующей группой.
    #[test]
    fn workspace_with_content_cannot_be_removed() {
        let (mut conn, path, users, ws) = test_db();
        let item = insert_item(&conn, ws, None, false, None);

        let attempt = dispatch(
            &mut conn,
            "admin.workspaces.remove",
            &json!({"id": ws}),
            Some(users[0]),
        );
        assert!(attempt.is_err(), "группу с предметами удалять нельзя");
        let alive: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspaces WHERE id=?1",
                params![ws],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alive, 1);

        conn.execute("DELETE FROM items WHERE id=?1", params![item])
            .unwrap();
        conn.execute(
            "DELETE FROM history_entries WHERE workspace_id=?1",
            params![ws],
        )
        .unwrap();
        dispatch(
            &mut conn,
            "admin.workspaces.remove",
            &json!({"id": ws}),
            Some(users[0]),
        )
        .expect("пустая группа удаляется");
        let members: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_workspaces WHERE workspace_id=?1",
                params![ws],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(members, 0, "членства не должны оставаться сиротами");
        cleanup(conn, path);
    }

    #[test]
    fn viewer_invite_grants_read_only_membership() {
        let (mut conn, path, users, ws) = test_db();
        let item = insert_item(&conn, ws, None, false, None);
        let created = dispatch(
            &mut conn,
            "admin.workspaces.createInvite",
            &json!({"workspaceId": ws, "role": "viewer", "maxUses": 5}),
            Some(users[0]),
        )
        .unwrap();
        let token = created["token"].as_str().unwrap().to_string();
        let joined = dispatch(
            &mut conn,
            "auth.joinRegister",
            &json!({"token": token, "fullName": "Наблюдатель", "phone": "+79995551111", "password": "LongEnoughPass1"}),
            None,
        )
        .unwrap();
        let viewer = joined["id"].as_i64().unwrap();

        let stored: String = conn
            .query_row(
                "SELECT rights_json FROM user_workspaces WHERE user_id=?1 AND workspace_id=?2",
                params![viewer, ws],
                |r| r.get(0),
            )
            .unwrap();
        let rights: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(rights["viewItems"].as_bool(), Some(true));
        assert_eq!(rights["createItems"].as_bool(), Some(false));

        // Наблюдатель читает каталог, но не создаёт и не берёт предметы —
        // даже если не передаёт workspaceId в запросе.
        dispatch(&mut conn, "items.byId", &json!({"id": item}), Some(viewer)).unwrap();
        let create = dispatch(
            &mut conn,
            "items.create",
            &json!({"title": "Чужой предмет"}),
            Some(viewer),
        )
        .unwrap_err();
        assert_eq!(create.http, 403);
        let take = dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": item}),
            Some(viewer),
        )
        .unwrap_err();
        assert_eq!(take.http, 403);
        cleanup(conn, path);
    }

    #[test]
    fn workspace_right_applies_when_request_omits_workspace_id() {
        let (mut conn, path, users, ws) = test_db();
        let limited = json!({"viewItems": true, "createItems": false});
        conn.execute(
            "UPDATE user_workspaces SET rights_json=?1 WHERE user_id=?2 AND workspace_id=?3",
            params![limited.to_string(), users[1], ws],
        )
        .unwrap();
        let error = dispatch(
            &mut conn,
            "items.create",
            &json!({"title": "Без workspaceId"}),
            Some(users[1]),
        )
        .unwrap_err();
        assert_eq!(error.http, 403);
        let items: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(items, 0);
        cleanup(conn, path);
    }

    #[test]
    fn status_change_to_repair_requires_a_reason() {
        let (mut conn, path, users, ws) = test_db();
        let item = insert_item(&conn, ws, None, false, None);
        conn.execute(
            "INSERT INTO statuses (name, workspace_id, type, slug) VALUES ('В ремонте',?1,'status','in-repair')",
            params![ws],
        )
        .unwrap();
        let repair = conn.last_insert_rowid();

        let refused = dispatch(
            &mut conn,
            "items.update",
            &json!({"id": item, "statusId": repair}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(refused.http, 400);
        let unchanged: Option<i64> = conn
            .query_row(
                "SELECT status_id FROM items WHERE id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unchanged, None);

        dispatch(
            &mut conn,
            "items.update",
            &json!({"id": item, "statusId": repair, "reason": "Сгорел якорь"}),
            Some(users[0]),
        )
        .unwrap();
        let applied: Option<i64> = conn
            .query_row(
                "SELECT status_id FROM items WHERE id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(applied, Some(repair));

        let note: String = conn
            .query_row(
                "SELECT comment FROM history_entries WHERE item_id=?1 ORDER BY id DESC LIMIT 1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert!(note.contains("В ремонте"), "{note}");
        assert!(note.contains("Сгорел якорь"), "{note}");
        cleanup(conn, path);
    }

    #[test]
    fn new_workspace_gets_statuses_and_a_storage() {
        let (mut conn, path, users, _ws) = test_db();
        let created = dispatch(
            &mut conn,
            "admin.workspaces.create",
            &json!({"name": "Второй объект"}),
            Some(users[0]),
        )
        .unwrap();
        let ws = created["id"].as_i64().unwrap();

        let mut stmt = conn
            .prepare("SELECT slug FROM statuses WHERE workspace_id=?1 ORDER BY slug")
            .unwrap();
        let slugs: Vec<String> = stmt
            .query_map(params![ws], |r| r.get(0))
            .unwrap()
            .filter_map(|x| x.ok())
            .collect();
        drop(stmt);
        for expected in [
            "in-work",
            "in-repair",
            "in-stock",
            "needs-check",
            "written-off",
        ] {
            assert!(
                slugs.iter().any(|s| s == expected),
                "{expected} missing from {slugs:?}"
            );
        }

        let storages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM storages WHERE workspace_id=?1",
                params![ws],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(storages, 1);

        // Предмет, взятый в новом пространстве, получает статус «В работе».
        let item = dispatch(
            &mut conn,
            "items.create",
            &json!({"workspaceId": ws, "title": "Новый предмет"}),
            Some(users[0]),
        )
        .unwrap();
        let taken = dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": item["id"].as_i64().unwrap()}),
            Some(users[0]),
        )
        .unwrap();
        assert_eq!(taken["status"]["slug"].as_str(), Some("in-work"));
        cleanup(conn, path);
    }

    #[test]
    fn return_rolls_back_when_ledger_append_fails() {
        let (mut conn, path, users, ws) = test_db();
        let item_id = insert_item(&conn, ws, Some(users[0]), false, None);
        break_ledger(&conn);

        let error =
            transfers_return(&mut conn, &json!({"itemId": item_id}), Some(users[0])).unwrap_err();
        assert_eq!(error.http, 500);

        // Предмет остался у сотрудника: возврат без записи в журнал не считается.
        let responsible: Option<i64> = conn
            .query_row(
                "SELECT responsible_user_id FROM items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(responsible, Some(users[0]));
        cleanup(conn, path);
    }

    #[test]
    fn quantity_return_adds_stock_back_and_journals_it() {
        let (mut conn, path, users, ws) = test_db();
        let item_id = insert_item(&conn, ws, None, true, Some(10.0));
        dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": item_id, "quantity": 4.0}),
            Some(users[1]),
        )
        .unwrap();
        let after_take: f64 = conn
            .query_row(
                "SELECT quantity FROM items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!((after_take - 6.0).abs() < 1e-9, "{after_take}");

        dispatch(
            &mut conn,
            "transfers.returnItem",
            &json!({"itemId": item_id, "quantity": 4.0}),
            Some(users[1]),
        )
        .unwrap();
        let after_return: f64 = conn
            .query_row(
                "SELECT quantity FROM items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!((after_return - 10.0).abs() < 1e-9, "{after_return}");

        let journaled: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_entries WHERE item_id=?1 AND type='transfer_send'",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(journaled, 1);
        cleanup(conn, path);
    }

    #[test]
    fn inventory_discrepancy_becomes_a_correcting_ledger_entry() {
        let (mut conn, path, users, ws) = test_db();
        let item = insert_item(&conn, ws, None, true, Some(10.0));
        let session = dispatch(
            &mut conn,
            "inventory.create",
            &json!({"workspaceId": ws}),
            Some(users[0]),
        )
        .unwrap();
        let sid = session["id"].as_i64().unwrap();

        dispatch(
            &mut conn,
            "inventory.checkItem",
            &json!({"sessionId": sid, "itemId": item, "checked": true, "actualQty": 7.0}),
            Some(users[0]),
        )
        .unwrap();
        let done = dispatch(
            &mut conn,
            "inventory.complete",
            &json!({"sessionId": sid}),
            Some(users[0]),
        )
        .unwrap();
        assert_eq!(done["corrections"].as_u64(), Some(1));

        // Остаток приведён к фактическому.
        let qty: f64 = conn
            .query_row(
                "SELECT quantity FROM items WHERE id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert!((qty - 7.0).abs() < 1e-9, "{qty}");

        // История сохранила корректировку с дельтой, а не переписала прошлое.
        let (delta, comment): (f64, String) = conn
            .query_row(
                "SELECT quantity_delta, comment FROM history_entries
                 WHERE item_id=?1 AND type='inventory' ORDER BY id DESC LIMIT 1",
                params![item],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!((delta + 3.0).abs() < 1e-9, "{delta}");
        assert!(comment.contains("фактически 7"), "{comment}");
        cleanup(conn, path);
    }

    #[test]
    fn auth_options_opens_registration_only_until_bootstrap() {
        let path = std::env::temp_dir().join(format!("meshkeeper-boot-{}.db", Uuid::new_v4()));
        let mut conn = db::open(&path).expect("test database");

        let empty = dispatch(&mut conn, "auth.options", &Value::Null, None).unwrap();
        assert_eq!(empty["registrationOpen"].as_bool(), Some(true));
        assert_eq!(empty["bootstrap"].as_bool(), Some(true));

        dispatch(
            &mut conn,
            "auth.register",
            &json!({"fullName": "Владелец", "phone": "+79990000000", "password": "LongEnoughPass1", "workspaceName": "Объект"}),
            None,
        )
        .unwrap();

        let after = dispatch(&mut conn, "auth.options", &Value::Null, None).unwrap();
        assert_eq!(after["registrationOpen"].as_bool(), Some(false));
        assert_eq!(after["bootstrap"].as_bool(), Some(false));
        cleanup(conn, path);
    }

    #[test]
    fn fault_and_change_requests_stay_inside_their_workspace() {
        let (mut conn, path, users, _ws) = test_db();
        // Второе пространство с собственным предметом и заявками.
        conn.execute(
            "INSERT INTO workspaces (name, timezone, internal_id_prefix, created_at) VALUES ('Other','UTC','O-',?1)",
            params![now()],
        )
        .unwrap();
        let other_ws = conn.last_insert_rowid();
        let other_item = insert_item(&conn, other_ws, None, false, None);
        conn.execute(
            "INSERT INTO faults (item_id, workspace_id, author_id, severity, description, created_at)
             VALUES (?1,?2,?3,'high','Чужая поломка',?4)",
            params![other_item, other_ws, users[0], now()],
        )
        .unwrap();
        let fault_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO change_requests (item_id, workspace_id, author_id, payload, created_at)
             VALUES (?1,?2,?3,'{\"title\":\"Взлом\"}',?4)",
            params![other_item, other_ws, users[0], now()],
        )
        .unwrap();
        let change_id = conn.last_insert_rowid();

        // users[0] состоит только в `ws`, но не в `other_ws`.
        let fault_err = dispatch(
            &mut conn,
            "items.resolveFault",
            &json!({"id": fault_id, "status": "resolved"}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(fault_err.http, 403);

        let change_err = dispatch(
            &mut conn,
            "items.decideChange",
            &json!({"id": change_id, "accept": true}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(change_err.http, 403);

        let title: String = conn
            .query_row(
                "SELECT title FROM items WHERE id=?1",
                params![other_item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Test item");
        cleanup(conn, path);
    }

    #[test]
    fn reported_fault_puts_item_under_review_and_blocks_checkout() {
        let (mut conn, path, users, ws) = test_db();
        seed_workspace_defaults(&conn, ws, users[0]).unwrap();
        let item = insert_item(&conn, ws, None, false, None);
        dispatch(
            &mut conn,
            "items.reportFault",
            &json!({"itemId": item, "severity": "high", "description": "Не держит патрон"}),
            Some(users[1]),
        )
        .unwrap();

        let slug: String = conn
            .query_row(
                "SELECT s.slug FROM items i JOIN statuses s ON s.id=i.status_id WHERE i.id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(slug, "needs-check");

        let blocked = dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": item}),
            Some(users[1]),
        )
        .unwrap_err();
        assert_eq!(blocked.http, 400);

        let journaled: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_entries WHERE item_id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(journaled, 1);
        cleanup(conn, path);
    }

    #[test]
    fn accepted_change_request_that_cannot_apply_is_not_marked_accepted() {
        let (mut conn, path, users, ws) = test_db();
        seed_workspace_defaults(&conn, ws, users[0]).unwrap();
        let item = insert_item(&conn, ws, None, false, None);
        let written_off: i64 = conn
            .query_row(
                "SELECT id FROM statuses WHERE workspace_id=?1 AND slug='written-off'",
                params![ws],
                |r| r.get(0),
            )
            .unwrap();
        // Заявка меняет статус на «Списан», но причины в ней нет.
        conn.execute(
            "INSERT INTO change_requests (item_id, workspace_id, author_id, payload, created_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![
                item,
                ws,
                users[1],
                json!({"statusId": written_off}).to_string(),
                now()
            ],
        )
        .unwrap();
        let change_id = conn.last_insert_rowid();

        // Причина берётся из решения администратора — заявка применяется.
        dispatch(
            &mut conn,
            "items.decideChange",
            &json!({"id": change_id, "accept": true, "reason": "Утилизирован по акту"}),
            Some(users[0]),
        )
        .unwrap();
        let status_id: Option<i64> = conn
            .query_row(
                "SELECT status_id FROM items WHERE id=?1",
                params![item],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status_id, Some(written_off));

        // Повторное решение по той же заявке отклоняется.
        let again = dispatch(
            &mut conn,
            "items.decideChange",
            &json!({"id": change_id, "accept": false}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(again.http, 409);
        cleanup(conn, path);
    }

    #[test]
    fn written_off_item_cannot_be_transferred() {
        let (mut conn, path, users, ws) = test_db();
        seed_workspace_defaults(&conn, ws, users[0]).unwrap();
        let item = insert_item(&conn, ws, Some(users[0]), false, None);
        let written_off: i64 = conn
            .query_row(
                "SELECT id FROM statuses WHERE workspace_id=?1 AND slug='written-off'",
                params![ws],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "UPDATE items SET status_id=?1 WHERE id=?2",
            params![written_off, item],
        )
        .unwrap();

        let error = dispatch(
            &mut conn,
            "transfers.prepare",
            &json!({"itemId": item, "toUserId": users[1]}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(error.http, 400);
        let transfers: i64 = conn
            .query_row("SELECT COUNT(*) FROM transfers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(transfers, 0);
        cleanup(conn, path);
    }

    #[test]
    fn removing_a_member_keeps_their_history() {
        let (mut conn, path, users, ws) = test_db();
        seed_workspace_defaults(&conn, ws, users[0]).unwrap();
        let item = insert_item(&conn, ws, None, false, None);
        dispatch(
            &mut conn,
            "transfers.take",
            &json!({"itemId": item}),
            Some(users[1]),
        )
        .unwrap();

        // Себя удалить нельзя.
        let self_remove = dispatch(
            &mut conn,
            "admin.users.remove",
            &json!({"id": users[0], "workspaceId": ws}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(self_remove.http, 400);

        let result = dispatch(
            &mut conn,
            "admin.users.remove",
            &json!({"id": users[1], "workspaceId": ws}),
            Some(users[0]),
        )
        .unwrap();
        assert_eq!(result["deleted"].as_bool(), Some(false));

        // Учётная запись заблокирована и выведена из пространства…
        let status: String = conn
            .query_row(
                "SELECT status FROM users WHERE id=?1",
                params![users[1]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "disabled");
        let member: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_workspaces WHERE user_id=?1 AND workspace_id=?2",
                params![users[1], ws],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(member, 0);

        // …но записи журнала остались на месте.
        let entries: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_entries WHERE actor_user_id=?1",
                params![users[1]],
                |r| r.get(0),
            )
            .unwrap();
        assert!(entries > 0);
        cleanup(conn, path);
    }

    #[test]
    fn repeated_bad_passwords_lock_the_account_out() {
        let (conn, path, users, _ws) = test_db();
        conn.execute(
            "UPDATE users SET password_hash=?1, phone='+79001234567' WHERE id=?2",
            params![hash_password("CorrectHorse1"), users[0]],
        )
        .unwrap();

        // Первые попытки просто отклоняются.
        for _ in 0..LOGIN_FREE_ATTEMPTS {
            let e = auth_login(
                &conn,
                &json!({"phone": "+7 900 123-45-67", "password": "wrong"}),
            )
            .unwrap_err();
            assert_eq!(e.http, 401, "{}", e.message);
        }

        // Дальше включается пауза.
        let locked = auth_login(
            &conn,
            &json!({"phone": "+7 900 123-45-67", "password": "wrong"}),
        )
        .unwrap_err();
        assert_eq!(locked.http, 429, "{}", locked.message);

        // Верный пароль в этот момент тоже не проходит — иначе паузу
        // можно было бы обойти, угадав с шестого раза.
        let blocked = auth_login(
            &conn,
            &json!({"phone": "+7 900 123-45-67", "password": "CorrectHorse1"}),
        )
        .unwrap_err();
        assert_eq!(blocked.http, 429);

        // После снятия блокировки вход работает и счётчик обнуляется.
        conn.execute("UPDATE login_throttle SET locked_until=NULL", [])
            .unwrap();
        auth_login(
            &conn,
            &json!({"phone": "+7 900 123-45-67", "password": "CorrectHorse1"}),
        )
        .unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM login_throttle", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        cleanup(conn, path);
    }

    #[test]
    fn change_request_shows_before_and_after() {
        let (mut conn, path, users, ws) = test_db();
        seed_workspace_defaults(&conn, ws, users[0]).unwrap();
        let item = insert_item(&conn, ws, None, false, None);
        conn.execute(
            "INSERT INTO categories (name, workspace_id, type) VALUES ('Электроинструмент',?1,'category')",
            params![ws],
        )
        .unwrap();
        let category = conn.last_insert_rowid();

        dispatch(
            &mut conn,
            "items.requestChange",
            &json!({
                "itemId": item,
                "payload": {"title": "Перфоратор Bosch", "categoryId": category},
                "comment": "уточнил модель"
            }),
            Some(users[1]),
        )
        .unwrap();

        let list = dispatch(
            &mut conn,
            "items.changeRequests",
            &json!({"workspaceId": ws}),
            Some(users[0]),
        )
        .unwrap();
        let changes = list[0]["changes"].as_array().expect("changes");
        assert_eq!(changes.len(), 2, "{changes:?}");

        let title = changes.iter().find(|c| c["field"] == "title").unwrap();
        assert_eq!(title["label"].as_str(), Some("Наименование"));
        assert_eq!(title["before"].as_str(), Some("Test item"));
        assert_eq!(title["after"].as_str(), Some("Перфоратор Bosch"));

        // Идентификатор категории развёрнут в название, а не показан числом.
        let cat = changes.iter().find(|c| c["field"] == "categoryId").unwrap();
        assert!(cat["before"].is_null());
        assert_eq!(cat["after"].as_str(), Some("Электроинструмент"));
        cleanup(conn, path);
    }

    #[test]
    fn change_request_hides_fields_that_do_not_change() {
        let (mut conn, path, users, ws) = test_db();
        let item = insert_item(&conn, ws, None, false, None);
        dispatch(
            &mut conn,
            "items.requestChange",
            &json!({"itemId": item, "payload": {"title": "Test item"}}),
            Some(users[1]),
        )
        .unwrap();
        let list = dispatch(
            &mut conn,
            "items.changeRequests",
            &json!({"workspaceId": ws}),
            Some(users[0]),
        )
        .unwrap();
        assert!(list[0]["changes"].as_array().unwrap().is_empty());
        cleanup(conn, path);
    }

    #[test]
    fn photo_and_location_rights_hide_fields_in_every_shape() {
        let (mut conn, path, users, ws) = test_db();
        seed_workspace_defaults(&conn, ws, users[0]).unwrap();
        let item = insert_item(&conn, ws, None, false, None);
        conn.execute(
            "INSERT INTO item_photos (item_id, url, is_title) VALUES (?1,'data:image/png;base64,AAA',1)",
            params![item],
        )
        .unwrap();
        let storage: i64 = conn
            .query_row(
                "SELECT id FROM storages WHERE workspace_id=?1",
                params![ws],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "UPDATE items SET storage_id=?1 WHERE id=?2",
            params![storage, item],
        )
        .unwrap();

        // Владелец видит всё.
        let full = dispatch(
            &mut conn,
            "items.byId",
            &json!({"id": item}),
            Some(users[0]),
        )
        .unwrap();
        assert!(!full["photos"].as_array().unwrap().is_empty());
        assert!(!full["storage"].is_null());

        // У второго пользователя оба права сняты.
        let limited = json!({"viewItems": true, "viewPhotos": false, "viewLocation": false});
        conn.execute(
            "UPDATE user_workspaces SET rights_json=?1 WHERE user_id=?2 AND workspace_id=?3",
            params![limited.to_string(), users[1], ws],
        )
        .unwrap();

        let hidden = dispatch(
            &mut conn,
            "items.byId",
            &json!({"id": item}),
            Some(users[1]),
        )
        .unwrap();
        assert!(hidden.get("photos").is_none(), "{hidden}");
        assert!(hidden.get("storage").is_none(), "{hidden}");
        assert!(hidden.get("storageId").is_none(), "{hidden}");
        // Само название и статус остаются — каталог смотреть можно.
        assert_eq!(hidden["title"].as_str(), Some("Test item"));

        // И во вложенных формах: список тоже вычищен.
        let list = dispatch(
            &mut conn,
            "reports.allItems",
            &json!({"workspaceId": ws}),
            Some(users[1]),
        )
        .unwrap();
        assert!(list[0].get("storage").is_none(), "{list}");
        cleanup(conn, path);
    }

    #[test]
    fn rights_saved_before_a_new_permission_existed_keep_working() {
        let (mut conn, path, users, ws) = test_db();
        let item = insert_item(&conn, ws, None, false, None);
        // Старая запись прав: про viewPhotos/viewLocation она ничего не знает.
        let legacy = json!({"viewItems": true, "createItems": true});
        conn.execute(
            "UPDATE user_workspaces SET rights_json=?1 WHERE user_id=?2 AND workspace_id=?3",
            params![legacy.to_string(), users[1], ws],
        )
        .unwrap();

        let seen = dispatch(
            &mut conn,
            "items.byId",
            &json!({"id": item}),
            Some(users[1]),
        )
        .unwrap();
        assert!(
            seen.get("photos").is_some(),
            "поле фото пропало у старой записи прав"
        );
        cleanup(conn, path);
    }

    #[test]
    fn write_off_photo_can_be_required_by_the_group() {
        let (mut conn, path, users, ws) = test_db();
        seed_workspace_defaults(&conn, ws, users[0]).unwrap();
        let first = insert_item(&conn, ws, None, false, None);
        let second = insert_item(&conn, ws, None, false, None);

        // По умолчанию фото не требуется — достаточно причины.
        dispatch(
            &mut conn,
            "history.writeOff",
            &json!({"itemId": first, "comment": "Сломан безвозвратно"}),
            Some(users[0]),
        )
        .unwrap();

        dispatch(
            &mut conn,
            "admin.workspaces.update",
            &json!({"id": ws, "requireWriteoffPhoto": true}),
            Some(users[0]),
        )
        .unwrap();

        let refused = dispatch(
            &mut conn,
            "history.writeOff",
            &json!({"itemId": second, "comment": "Утилизирован"}),
            Some(users[0]),
        )
        .unwrap_err();
        assert_eq!(refused.http, 400, "{}", refused.message);

        dispatch(
            &mut conn,
            "history.writeOff",
            &json!({
                "itemId": second,
                "comment": "Утилизирован по акту",
                "photoUrl": "data:image/png;base64,AAAA"
            }),
            Some(users[0]),
        )
        .unwrap();

        // Фото сохранено при записи журнала, а не потеряно.
        let stored: Option<String> = conn
            .query_row(
                "SELECT photo_url FROM history_entries WHERE item_id=?1 AND type='write_off'",
                params![second],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some("data:image/png;base64,AAAA"));
        cleanup(conn, path);
    }

    #[test]
    fn photos_get_a_thumbnail_and_a_checksum() {
        let (mut conn, path, users, ws) = test_db();
        let full = "data:image/jpeg;base64,QQQQQQQQQQQQ";
        let thumb = "data:image/jpeg;base64,VGh1bWI=";

        let created = dispatch(
            &mut conn,
            "items.create",
            &json!({
                "workspaceId": ws,
                "title": "Перфоратор",
                "photos": [{"url": full, "thumbUrl": thumb}]
            }),
            Some(users[0]),
        )
        .unwrap();
        let item = created["id"].as_i64().unwrap();

        let photo = &created["photos"][0];
        assert_eq!(photo["url"].as_str(), Some(full));
        assert_eq!(photo["thumbUrl"].as_str(), Some(thumb));
        // Контрольная сумма считается сервером, а не приходит от клиента.
        let expected = photo_checksum(full);
        assert_eq!(photo["sha256"].as_str(), Some(expected.as_str()));
        assert_eq!(expected.len(), 64);

        // В списке каталога оригинал не отдаётся — только миниатюра.
        let list = dispatch(
            &mut conn,
            "items.list",
            &json!({"workspaceId": ws}),
            Some(users[0]),
        )
        .unwrap();
        let listed = &list["rows"][0]["photos"][0];
        assert_eq!(
            listed["url"].as_str(),
            Some(thumb),
            "в списке уехал оригинал"
        );
        assert_eq!(listed["thumbUrl"].as_str(), Some(thumb));

        // В карточке оригинал по-прежнему доступен.
        let card = dispatch(
            &mut conn,
            "items.byId",
            &json!({"id": item}),
            Some(users[0]),
        )
        .unwrap();
        assert_eq!(card["photos"][0]["url"].as_str(), Some(full));
        cleanup(conn, path);
    }

    #[test]
    fn photos_added_as_plain_strings_still_work() {
        let (mut conn, path, users, ws) = test_db();
        let url = "data:image/png;base64,AAAA";
        let created = dispatch(
            &mut conn,
            "items.create",
            &json!({"workspaceId": ws, "title": "Дрель", "photos": [url]}),
            Some(users[0]),
        )
        .unwrap();
        let photo = &created["photos"][0];
        assert_eq!(photo["url"].as_str(), Some(url));
        // Миниатюры нет — подставляется оригинал, карточка не остаётся пустой.
        assert_eq!(photo["thumbUrl"].as_str(), Some(url));
        assert!(photo["sha256"].as_str().is_some());
        cleanup(conn, path);
    }
}
