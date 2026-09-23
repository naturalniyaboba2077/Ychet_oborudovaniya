mod api;
mod auth;
mod db;
mod google;
mod json;
mod ledger;
mod sync;
mod update;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tower_http::services::ServeDir;

struct AppState {
    db: Mutex<Connection>,
}

fn unwrap_json(v: &Value) -> Value {
    if let Some(inner) = v.get("json") {
        if inner.get("json").is_some() && inner.get("meta").is_some() {
            return inner.get("json").cloned().unwrap_or(Value::Null);
        }
        return inner.clone();
    }
    v.clone()
}

fn parse_calls(
    procedures: &str,
    query_input: Option<&str>,
    body: Option<&[u8]>,
) -> Vec<(String, Value)> {
    let names: Vec<String> = procedures
        .split(',')
        .map(|s| s.trim().trim_start_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let raw: Option<Value> = body
        .and_then(|b| {
            if b.is_empty() {
                None
            } else {
                serde_json::from_slice(b).ok()
            }
        })
        .or_else(|| query_input.and_then(|s| serde_json::from_str(s).ok()));
    match raw {
        None => names.into_iter().map(|n| (n, Value::Null)).collect(),
        Some(Value::Object(map))
            if map.contains_key("0") || map.keys().any(|k| k.parse::<usize>().is_ok()) =>
        {
            names
                .into_iter()
                .enumerate()
                .map(|(i, n)| {
                    let inp = map.get(&i.to_string()).cloned().unwrap_or(Value::Null);
                    (n, unwrap_json(&inp))
                })
                .collect()
        }
        Some(v) => {
            let inp = unwrap_json(&v);
            if names.len() == 1 {
                vec![(names[0].clone(), inp)]
            } else {
                names.into_iter().map(|n| (n, inp.clone())).collect()
            }
        }
    }
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    if let Some(cookie) = headers.get("cookie").and_then(|h| h.to_str().ok()) {
        for part in cookie.split(';') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix("mk_session=") {
                return Some(v);
            }
        }
    }
    None
}

fn ok_payload(data: Value) -> Value {
    json!({"result": {"data": {"json": data}}})
}

fn err_payload(e: &api::ApiError) -> Value {
    json!({
        "error": {
            "json": {
                "message": e.message,
                "code": match e.http { 401 => -32001, 403 => -32003, 404 => -32004, 409 => -32009, _ => -32603 },
                "data": { "code": e.code, "httpStatus": e.http }
            }
        }
    })
}

async fn trpc(
    State(state): State<Arc<AppState>>,
    Path(procedures): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let calls = parse_calls(&procedures, q.get("input").map(|s| s.as_str()), Some(&body));
    let has_mutation = calls
        .iter()
        .any(|(procedure, _)| api::is_mutation(procedure));
    if method == Method::GET && has_mutation {
        return (StatusCode::METHOD_NOT_ALLOWED, "mutations require POST").into_response();
    }
    if method != Method::GET && has_mutation {
        let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
            return (
                StatusCode::FORBIDDEN,
                "mutation requires same-origin Origin header",
            )
                .into_response();
        };
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        let allowed = origin == format!("http://{host}") || origin == format!("https://{host}");
        if !allowed {
            return (StatusCode::FORBIDDEN, "cross-site request rejected").into_response();
        }
    }
    // Адрес клиента приходит от обратного прокси. Берём первый элемент
    // X-Forwarded-For: остальные дописывают промежуточные узлы, и доверять
    // им нельзя. Заголовок подделывается, поэтому это заслон от перебора,
    // а не доказательство личности.
    let client_addr = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    api::set_client_address(&client_addr);
    api::set_active_workspace(
        headers
            .get("x-mk-workspace")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok()),
    );

    let token = session_token(&headers).map(str::to_owned);
    let batched = calls.len() > 1 || q.get("batch").map(|s| s.as_str()) == Some("1");
    let mut conn = state.db.lock();
    let uid = auth::resolve_session(&conn, token.as_deref());
    let mut out = Vec::new();
    let mut set_session: Option<Option<String>> = None;
    for (proc, input) in &calls {
        match api::dispatch(&mut conn, proc, input, uid) {
            Ok(data) => {
                if proc == "auth.login" || proc == "auth.register" || proc == "auth.joinRegister" {
                    if let Some(id) = data.get("id").and_then(|v| v.as_i64()) {
                        match auth::create_session(&conn, id) {
                            Ok(new_token) => set_session = Some(Some(new_token)),
                            Err(e) => {
                                out.push(err_payload(&api::ApiError::internal(format!(
                                    "Не удалось создать сессию: {e}"
                                ))));
                                continue;
                            }
                        }
                    }
                }
                if proc == "auth.logout" {
                    let _ = auth::revoke_session(&conn, token.as_deref());
                    set_session = Some(None);
                }
                log_event(proc, uid, None);
                out.push(ok_payload(data));
            }
            Err(e) => {
                log_event(proc, uid, Some(&e));
                out.push(err_payload(&e));
            }
        }
    }
    let body = if batched || out.len() != 1 {
        Value::Array(out)
    } else {
        out.pop().unwrap_or(json!({}))
    };
    let mut builder = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json; charset=utf-8");
    if let Some(session) = set_session {
        let secure = std::env::var("MESHKEEPER_COOKIE_SECURE").as_deref() == Ok("1");
        let cookie = match session {
            Some(token) => format!(
                "mk_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000{}",
                if secure { "; Secure" } else { "" }
            ),
            None => format!(
                "mk_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
                if secure { "; Secure" } else { "" }
            ),
        };
        builder = builder.header("set-cookie", cookie);
    }
    builder.body(body.to_string()).unwrap().into_response()
}

/// Велит браузеру перепроверять файл, а не доставать его из своих запасов.
///
/// Заголовка не было вовсе, и браузер решал сам: после выкладки человек
/// продолжал открывать старую сборку. В приложении, где страницу вручную
/// никто не обновляет, это тянулось бы неделями.
///
/// Файлам в `/assets/` напрашивается «хранить год, не перепроверять» — их
/// имена выглядят как отпечаток содержимого. Но это проверено и оказалось
/// неправдой: сборщик выдал одно и то же имя `index-DhbuBn33.js` для двух
/// разных сборок, и браузер показывал старый каталог, имея на руках новый.
/// С «immutable» он держал бы этот файл год, и починить это снаружи было бы
/// нечем. Поэтому перепроверяется всё; ответ при этом почти всегда пустой
/// (304 по дате изменения), так что стоит это мало.
async fn static_cache_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    res
}

async fn spa_index(State(frontend): State<Arc<update::Frontend>>, uri: Uri) -> Response {
    // Отсутствующий ассет должен оставаться 404, иначе сломанный бандл
    // возвращает HTML вместо скрипта и ошибка становится незаметной.
    let path = uri.path();
    let looks_like_file = path
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'));
    if path.starts_with("/assets/") || looks_like_file {
        return (StatusCode::NOT_FOUND, "Файл не найден").into_response();
    }
    update::index_response(&frontend).await
}

/// Возвращение от Google. Отдаёт HTML, а не редирект, намеренно: сессионная
/// cookie помечена SameSite=Strict, и при переходе, начатом на стороне
/// google.com, браузер её на следующий запрос не пошлёт. Страница же уводит
/// на приложение уже своим переходом — он считается своим, и cookie доедет.
async fn google_callback(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if let Some(error) = q.get("error") {
        return google_page(None, &format!("Google отказал: {error}"));
    }
    let (Some(code), Some(oauth_state)) = (q.get("code"), q.get("state")) else {
        return google_page(None, "Google вернул неполный ответ");
    };
    // Замок держим только на время работы с базой: обмен кода ходит в сеть,
    // и удерживать соединение всё это время нельзя — встанут другие запросы.
    let pending = {
        let conn = state.db.lock();
        google::take_pending(&conn, oauth_state)
    };
    let Some(pending) = pending else {
        return google_page(None, "Ссылка устарела, попробуйте войти заново");
    };
    let identity = match google::exchange(code).await {
        Ok(v) => v,
        Err(e) => return google_page(None, &format!("Не удалось проверить аккаунт: {e}")),
    };
    let issued = {
        let conn = state.db.lock();
        match api::google_finish(&conn, &identity, &pending) {
            Ok(uid) => auth::create_session(&conn, uid)
                .map_err(|e| api::ApiError::internal(format!("Не удалось создать сессию: {e}"))),
            Err(e) => Err(e),
        }
    };
    match issued {
        Ok(token) => google_page(Some(&token), ""),
        Err(e) => google_page(None, &e.message),
    }
}

/// Страница-переходник. При успехе ставит cookie и уводит в приложение,
/// при отказе показывает причину и ссылку назад на вход.
fn google_page(session: Option<&str>, error: &str) -> axum::response::Response {
    let body = if error.is_empty() {
        "<!doctype html><meta charset=\"utf-8\"><title>Вход выполнен</title>\
         <p style=\"font:16px system-ui;margin:3rem\">Входим…</p>\
         <script>location.replace('/')</script>"
            .to_string()
    } else {
        format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Вход не удался</title>\
             <div style=\"font:16px system-ui;margin:3rem;max-width:34rem\">\
             <h1 style=\"font-size:1.25rem\">Войти через Google не получилось</h1>\
             <p>{}</p><p><a href=\"/login\">Вернуться ко входу</a></p></div>",
            html_escape(error)
        )
    };
    let mut builder = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8");
    if let Some(token) = session {
        let secure = std::env::var("MESHKEEPER_COOKIE_SECURE").as_deref() == Ok("1");
        builder = builder.header(
            "set-cookie",
            format!(
                "mk_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000{}",
                if secure { "; Secure" } else { "" }
            ),
        );
    }
    builder
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Текст ошибки приходит в том числе от Google — в разметку его без экранирования пускать нельзя.
fn html_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Пишет в журнал сервиса то, что нужно при разборе инцидента.
///
/// Раньше в journald попадало только сообщение о старте: ни входов, ни
/// отказов, ни ошибок обмена. Для системы, от которой требуют аудит, это
/// странно — когда спросят «кто и когда», отвечать будет нечем.
///
/// Пишем не всё подряд: успешные чтения дают шум, в котором тонет важное.
/// В журнал идут изменения, попытки входа и любые отказы.
fn log_event(procedure: &str, uid: Option<i64>, error: Option<&api::ApiError>) {
    let interesting =
        api::is_mutation(procedure) || procedure.starts_with("auth.") || error.is_some();
    if !interesting {
        return;
    }
    let who = match uid {
        Some(id) => format!("user:{id}"),
        None => "аноним".to_string(),
    };
    match error {
        // Текста ошибки достаточно: персональных данных в нём нет, а
        // причина отказа видна.
        Some(e) => eprintln!(
            "{} ОТКАЗ {procedure} {who} {} {}",
            chrono::Utc::now().to_rfc3339(),
            e.code,
            e.message
        ),
        None => eprintln!("{} ok {procedure} {who}", chrono::Utc::now().to_rfc3339()),
    }
}

/// Предельный размер запроса. Карточка с несколькими снимками в это
/// укладывается; заливка диска — нет.
const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;

/// Отдаёт вложение по имени файла.
///
/// Раздаём сами, а не готовым каталогом: снимки закрыты правом «видеть
/// фотографии», и публичная раздача обошла бы его — тот, у кого право
/// отобрали, продолжал бы качать по сохранённой ссылке. Плюс имя проверяется
/// по строгому шаблону, поэтому выйти за пределы каталога нечем.
async fn serve_attachment(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Имя — контрольная сумма и расширение. Ни путей, ни точек, ни слэшей.
    let (stem, ext) = match name.rsplit_once('.') {
        Some(v) => v,
        None => return (StatusCode::NOT_FOUND, "нет такого файла").into_response(),
    };
    let sane = stem.len() == 64
        && stem.bytes().all(|b| b.is_ascii_hexdigit())
        && (1..=4).contains(&ext.len())
        && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    if !sane {
        return (StatusCode::NOT_FOUND, "нет такого файла").into_response();
    }

    {
        let conn = state.db.lock();
        let token = session_token(&headers);
        if auth::resolve_session(&conn, token).is_none() {
            return (StatusCode::UNAUTHORIZED, "нужен вход").into_response();
        }
    }

    let path = api::files_dir().join(&name);
    let Ok(bytes) = std::fs::read(&path) else {
        return (StatusCode::NOT_FOUND, "нет такого файла").into_response();
    };
    // Снимки показываем прямо на странице, всё остальное отдаём файлом.
    // Содержимое пришло от людей: показать его в нашем же источнике —
    // значит позволить чужому PDF или HTML выполниться как своему.
    let (mime, inline) = match ext {
        "png" => ("image/png", true),
        "webp" => ("image/webp", true),
        "gif" => ("image/gif", true),
        "jpg" | "jpeg" => ("image/jpeg", true),
        "pdf" => ("application/pdf", false),
        "doc" => ("application/msword", false),
        "docx" => (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            false,
        ),
        "xls" => ("application/vnd.ms-excel", false),
        "xlsx" => (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            false,
        ),
        "txt" => ("text/plain; charset=utf-8", false),
        "csv" => ("text/csv; charset=utf-8", false),
        "zip" => ("application/zip", false),
        "rtf" => ("application/rtf", false),
        _ => ("application/octet-stream", false),
    };
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", mime)
        // Имя файла — сумма содержимого, поэтому по одному адресу всегда
        // одно и то же. Кэш приватный: ответ зависит от сессии.
        .header("cache-control", "private, max-age=31536000, immutable")
        .header("x-content-type-options", "nosniff")
        .header(
            "content-disposition",
            if inline { "inline" } else { "attachment" },
        )
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn health() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "node": "meshkeeper-node",
        "journal": "audit-log",
        "role": node_role(),
        "sync": if sync_token().is_some() { "enabled" } else { "disabled" },
    }))
}

/// Роль узла определяется конфигурацией, отдельного переключателя не нужно:
/// есть upstream — это локальный узел, нет upstream, но есть токен — сервер.
fn node_role() -> &'static str {
    match (upstream_url(), sync_token()) {
        (Some(_), _) => "node",
        (None, Some(_)) => "server",
        (None, None) => "standalone",
    }
}

fn upstream_url() -> Option<String> {
    std::env::var("MESHKEEPER_UPSTREAM")
        .ok()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
}

/// Общий секрет сервера и локальных узлов. Не задан — обмен выключен.
fn sync_token() -> Option<String> {
    std::env::var("MESHKEEPER_SYNC_TOKEN")
        .ok()
        .filter(|t| t.chars().count() >= 32)
}

fn sync_authorized(headers: &HeaderMap) -> bool {
    let Some(secret) = sync_token() else {
        return false;
    };
    let expected = format!("Bearer {secret}");
    let got = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    // Постоянное по времени сравнение: длина токена не секрет, содержимое — да.
    got.len() == expected.len()
        && got
            .as_bytes()
            .iter()
            .zip(expected.as_bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

async fn sync_hello(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if !sync_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"синхронизация выключена или неверный токен"})),
        )
            .into_response();
    }
    let db = state.db.lock();
    Json(sync::hello(&db)).into_response()
}

async fn sync_journal_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !sync_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"sync disabled"})),
        )
            .into_response();
    }
    let db = state.db.lock();
    Json(sync::export_journal(&db)).into_response()
}

async fn sync_journal_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !sync_authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"sync disabled"})),
        )
            .into_response();
    }
    let db = state.db.lock();
    let from = body
        .get("nodeId")
        .and_then(|v| v.as_str())
        .unwrap_or("peer");
    Json(sync::apply_remote_journal(&db, &body, from)).into_response()
}

/// Локальный узел обменивается изменениями с центральным сервером.
///
/// Работает офлайн-first: если сервер недоступен, узел продолжает работать на
/// своей базе, ошибка попадает в «Админка → Офлайн-узлы», а следующая попытка
/// произойдёт на следующем тике.
async fn upstream_loop(state: Arc<AppState>, upstream: String, token: String) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Синхронизация не запущена: {e}");
            return;
        }
    };
    let interval = std::env::var("MESHKEEPER_SYNC_INTERVAL")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(15)
        .clamp(5, 3600);
    eprintln!("Синхронизация с {upstream} каждые {interval} с");
    let mut waited = interval; // первый проход — сразу после старта
    loop {
        let asked_now = sync::take_sync_request();
        if asked_now || waited >= interval {
            sync_once(&client, &state, &upstream, &token).await;
            waited = 0;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        waited += 1;
    }
}

async fn sync_once(client: &reqwest::Client, state: &Arc<AppState>, upstream: &str, token: &str) {
    {
        let db = state.db.lock();
        sync::ensure_node(&db);
        sync::add_peer(&db, upstream, Some("Сервер"), None);
    }

    // Прежде чем гонять снимки, спрашиваем у сервера примету его состояния.
    // Если она не менялась с прошлого раза и наша тоже — обмениваться нечем,
    // и можно не тащить всю базу туда-обратно.
    let local_tag = {
        let db = state.db.lock();
        sync::state_tag(&db)
    };
    if let Ok(resp) = client
        .get(format!("{upstream}/sync/hello"))
        .bearer_auth(token)
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(info) = resp.json::<Value>().await {
                let remote_tag = info
                    .get("stateTag")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let unchanged = {
                    let db = state.db.lock();
                    let seen_remote = sync::kv_get(&db, "sync_seen_remote_tag");
                    let seen_local = sync::kv_get(&db, "sync_seen_local_tag");
                    !remote_tag.is_empty()
                        && seen_remote.as_deref() == Some(remote_tag.as_str())
                        && seen_local.as_deref() == Some(local_tag.as_str())
                };
                if unchanged {
                    let db = state.db.lock();
                    let _ = db.execute(
                        "UPDATE peers SET last_sync=?1, last_error=NULL WHERE url=?2",
                        rusqlite::params![chrono::Utc::now().to_rfc3339(), upstream],
                    );
                    return;
                }
            }
        }
    }

    // 1. Забираем изменения сервера.
    let pulled = client
        .get(format!("{upstream}/sync/journal"))
        .bearer_auth(token)
        .send()
        .await;
    match pulled {
        Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
            Ok(journal) => {
                let db = state.db.lock();
                sync::apply_remote_journal(&db, &journal, upstream);
            }
            Err(e) => {
                let db = state.db.lock();
                sync::touch_peer_error(&db, upstream, &format!("некорректный ответ: {e}"));
                return;
            }
        },
        Ok(resp) => {
            let status = resp.status();
            let db = state.db.lock();
            sync::touch_peer_error(&db, upstream, &format!("сервер ответил {status}"));
            return;
        }
        Err(e) => {
            let db = state.db.lock();
            sync::touch_peer_error(&db, upstream, &short_net_error(&e));
            return;
        }
    }

    // 2. Отдаём свои.
    let mine = {
        let db = state.db.lock();
        sync::export_journal(&db)
    };
    match client
        .post(format!("{upstream}/sync/journal"))
        .bearer_auth(token)
        .json(&mine)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let db = state.db.lock();
            let _ = db.execute(
                "UPDATE peers SET last_sync=?1, last_error=NULL WHERE url=?2",
                rusqlite::params![chrono::Utc::now().to_rfc3339(), upstream],
            );
            sync::kv_set(&db, "sync_seen_local_tag", &sync::state_tag(&db));
        }
        Ok(resp) => {
            let status = resp.status();
            let db = state.db.lock();
            sync::touch_peer_error(
                &db,
                upstream,
                &format!("сервер отклонил выгрузку: {status}"),
            );
        }
        Err(e) => {
            let db = state.db.lock();
            sync::touch_peer_error(&db, upstream, &short_net_error(&e));
            return;
        }
    }

    // Обмен состоялся — запоминаем, каким сервер стал после наших записей.
    // Пока обе приметы не изменятся, следующие круги пропускаются целиком.
    if let Ok(resp) = client
        .get(format!("{upstream}/sync/hello"))
        .bearer_auth(token)
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(info) = resp.json::<Value>().await {
                if let Some(tag) = info.get("stateTag").and_then(Value::as_str) {
                    let db = state.db.lock();
                    sync::kv_set(&db, "sync_seen_remote_tag", tag);
                    sync::kv_set(&db, "sync_seen_local_tag", &sync::state_tag(&db));
                }
            }
        }
    }
}

fn short_net_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "сервер не ответил вовремя".into()
    } else if e.is_connect() {
        "нет связи с сервером".into()
    } else {
        e.to_string()
    }
}

#[tokio::main]
async fn main() {
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let db_path = std::env::var("MESHKEEPER_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("data").join("meshkeeper-rs.db"));
    eprintln!("Узел MeshKeeper, база {}", db_path.display());
    // Понятное сообщение вместо трассировки паники: сюда попадают и обычные
    // ошибки доступа к файлу, и неверный ключ шифрования.
    let conn = match db::open(&db_path) {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("Не удалось открыть базу {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };
    {
        let _ = sync::ensure_node(&conn);
        eprintln!(
            "Узел {}, LAN {}",
            sync::kv_get(&conn, "node_name").unwrap_or_default(),
            sync::guess_lan_base()
        );
    }
    let state = Arc::new(AppState {
        db: Mutex::new(conn),
    });
    match (upstream_url(), sync_token()) {
        (Some(upstream), Some(token)) => {
            tokio::spawn(upstream_loop(state.clone(), upstream, token));
        }
        (Some(_), None) => {
            panic!("MESHKEEPER_UPSTREAM требует MESHKEEPER_SYNC_TOKEN не короче 32 символов")
        }
        (None, Some(_)) => eprintln!("Режим сервера: принимаю обмен на /sync/journal"),
        (None, None) => eprintln!("Автономный режим: обмен с сервером выключен"),
    }
    let web_root = std::env::var("MESHKEEPER_WEB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("dist").join("public"));
    // Маршруты SPA (/tool/1, /join?token=…) должны отдавать index.html со
    // статусом 200: ServeFile как not_found_service сохранял 404, из-за чего
    // ссылка-приглашение выглядела как «страница не найдена».
    // Следит за каталогом сборки и сообщает открытым клиентам о новой
    // версии интерфейса — см. update.rs.
    let frontend = update::start(web_root.clone());
    let static_files = ServeDir::new(&web_root)
        .fallback(any(spa_index).with_state(frontend.clone()));
    // Заголовки кэша приходится навешивать слоем: ServeDir их не ставит
    // вовсе, и браузер решает сам. На странице это оборачивалось тем, что
    // после выкладки человек продолжал открывать старую сборку — особенно
    // в приложении, где страницу никто не обновляет вручную.
    let static_files = Router::new()
        .fallback_service(static_files)
        .layer(axum::middleware::from_fn(static_cache_headers));
    let app = Router::new()
        .route("/health", get(health))
        .route("/sync/hello", get(sync_hello))
        .route(
            "/sync/journal",
            get(sync_journal_get).post(sync_journal_post),
        )
        .route("/auth/google/callback", get(google_callback))
        .route("/files/{name}", get(serve_attachment))
        .route("/api/trpc/{*procedures}", any(trpc))
        .merge(update::routes(frontend))
        // Предел на размер запроса. Без него любой желающий заливает сколько
        // угодно: тело читается в память целиком, а снимки теперь ещё и
        // ложатся на диск. Двадцать мегабайт — с запасом на карточку с
        // несколькими фотографиями, но не на заполнение диска.
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .fallback_service(static_files)
        .with_state(state);
    let addr = std::env::var("MESHKEEPER_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let loopback =
        addr.starts_with("127.") || addr.starts_with("localhost") || addr.starts_with("[::1]");
    if !loopback && std::env::var("MESHKEEPER_COOKIE_SECURE").as_deref() != Ok("1") {
        panic!("non-loopback bind requires MESHKEEPER_COOKIE_SECURE=1 and an HTTPS reverse proxy");
    }
    eprintln!("Слушаю {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Разбор батчей нетривиален и до сих пор не проверялся ни одним тестом,
    /// хотя через него проходит каждый запрос клиента.
    #[test]
    fn parse_calls_handles_single_batched_and_empty() {
        // Одиночный вызов: тело — объект с полем json (формат superjson).
        let one = parse_calls("auth.login", None, Some(br#"{"json":{"phone":"+7"}}"#));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].0, "auth.login");
        assert_eq!(one[0].1["phone"], json!("+7"));

        // Батч: имена через запятую, вход — по числовым ключам.
        let many = parse_calls(
            "auth.me,meta.workspaces",
            None,
            Some(br#"{"0":{"json":null},"1":{"json":{"a":1}}}"#),
        );
        assert_eq!(many.len(), 2);
        assert_eq!(many[0].0, "auth.me");
        assert_eq!(many[1].0, "meta.workspaces");
        assert_eq!(many[1].1["a"], json!(1));

        // Пустое тело: процедуры вызываются без входа.
        let none = parse_calls("auth.options", None, None);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].1, Value::Null);

        // Вход из строки запроса, когда тела нет (GET-запросы клиента).
        let from_query = parse_calls("items.list", Some(r#"{"0":{"json":{"page":2}}}"#), None);
        assert_eq!(from_query[0].1["page"], json!(2));

        // Мусор вместо JSON не должен ронять разбор.
        let broken = parse_calls("auth.me", None, Some(b"{not json"));
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].1, Value::Null);

        // Пустые и «/»-префиксные имена отбрасываются, а не превращаются
        // в вызов несуществующей процедуры.
        let dirty = parse_calls("/auth.me,,  ", None, None);
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].0, "auth.me");
    }

    /// Cookie сессии читается из общей строки, где лежат и чужие значения.
    #[test]
    fn session_token_is_picked_out_of_a_shared_cookie_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            "theme=dark; mk_session=abc123; other=1".parse().unwrap(),
        );
        assert_eq!(session_token(&headers), Some("abc123"));

        let mut only_foreign = HeaderMap::new();
        only_foreign.insert("cookie", "theme=dark".parse().unwrap());
        assert_eq!(session_token(&only_foreign), None);

        // Имя, оканчивающееся на mk_session, не должно подходить.
        let mut lookalike = HeaderMap::new();
        lookalike.insert("cookie", "not_mk_session=hack".parse().unwrap());
        assert_eq!(lookalike.len(), 1);
        assert_ne!(session_token(&lookalike), Some("hack"));
    }

    /// Разделение «читает» и «меняет» решает, нужна ли проверка Origin.
    /// Ошибка здесь открыла бы изменения для запросов с чужих сайтов.
    #[test]
    fn mutations_are_recognised_for_the_origin_check() {
        for p in [
            "auth.login",
            "auth.register",
            "items.create",
            "transfers.take",
            "admin.users.update",
        ] {
            assert!(api::is_mutation(p), "{p} должна считаться изменяющей");
        }
        for p in ["auth.me", "items.list", "meta.workspaces", "auth.options"] {
            assert!(!api::is_mutation(p), "{p} только читает");
        }
    }
}
