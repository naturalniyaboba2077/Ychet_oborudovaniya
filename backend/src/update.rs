//! Мгновенная доставка новой сборки интерфейса на открытые устройства.
//!
//! Установленное приложение (PWA) почти никогда не перезагружается: телефон
//! будит его из фона со старым кодом в памяти, и после выкладки человек
//! работает в прежней версии сутками. Заголовки кэша тут не помогают — до
//! запроса страницы дело просто не доходит.
//!
//! Поэтому сервер сам следит за каталогом сборки и сообщает об изменении
//! всем открытым клиентам по SSE. Клиентский скрипт (`update.js`) сервер
//! вставляет в `index.html` при отдаче, а не держит в исходниках фронтенда:
//! так он переживает любую пересборку интерфейса.
//!
//! Всё живёт под `/api/`: service worker этот префикс не трогает. Иначе он
//! пытался бы положить бесконечный поток событий в свой кэш.

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Json, Router,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

/// Как часто перечитывать каталог сборки. Это ~80 записей каталога,
/// так что опрос почти бесплатен, а задержка оповещения — пара секунд.
const POLL: Duration = Duration::from_secs(1);

const CLIENT_JS: &str = include_str!("update.js");

pub struct Frontend {
    web_root: PathBuf,
    build: watch::Receiver<String>,
}

/// Снимает отпечаток текущей сборки и запускает слежение за каталогом.
pub fn start(web_root: PathBuf) -> Arc<Frontend> {
    let initial = fingerprint(&web_root).unwrap_or_default();
    let (tx, rx) = watch::channel(initial);
    tokio::spawn(watch_loop(web_root.clone(), tx));
    Arc::new(Frontend {
        web_root,
        build: rx,
    })
}

/// Маршруты: сама страница и служебные адреса для клиентского скрипта.
pub fn routes<S>(frontend: Arc<Frontend>) -> Router<S> {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/api/app/version", get(version))
        .route("/api/app/events", get(events))
        .route("/api/app/update.js", get(client_js))
        .with_state(frontend)
}

async fn index(State(frontend): State<Arc<Frontend>>) -> Response {
    index_response(&frontend).await
}

/// `index.html` с подключённым скриптом обновления и меткой сборки,
/// из которой страница загружена.
pub async fn index_response(frontend: &Frontend) -> Response {
    let path = frontend.web_root.join("index.html");
    let Ok(html) = tokio::fs::read_to_string(&path).await else {
        return (
            StatusCode::NOT_FOUND,
            "UI не собран. Выполните: npm run build",
        )
            .into_response();
    };
    let build = frontend.build.borrow().clone();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        inject(&html, &build),
    )
        .into_response()
}

fn inject(html: &str, build: &str) -> String {
    // Отпечаток — hex, экранировать нечего.
    let tag = format!(
        "<script>window.__MK_BUILD=\"{build}\"</script>\
         <script src=\"/api/app/update.js\" defer></script>"
    );
    match html.find("</head>") {
        Some(at) => format!("{}{tag}{}", &html[..at], &html[at..]),
        None => format!("{html}{tag}"),
    }
}

async fn version(State(frontend): State<Arc<Frontend>>) -> impl IntoResponse {
    let build = frontend.build.borrow().clone();
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "build": build })),
    )
}

/// Поток SSE: текущая сборка сразу при подключении, затем каждая новая.
async fn events(State(frontend): State<Arc<Frontend>>) -> impl IntoResponse {
    let rx = frontend.build.clone();
    let stream = futures_util::stream::unfold((rx, true), |(mut rx, first)| async move {
        if !first && rx.changed().await.is_err() {
            return None;
        }
        let build = rx.borrow_and_update().clone();
        let event = Event::default().event("build").data(build);
        Some((Ok::<_, Infallible>(event), (rx, false)))
    });
    (
        // Прокси не должен копить поток у себя — иначе «мгновенно»
        // превращается в «когда буфер наполнится».
        [("x-accel-buffering", "no")],
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))),
    )
}

async fn client_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        CLIENT_JS,
    )
}

/// Объявляет новую сборку, только когда она выложена целиком.
///
/// Копирование сборки занимает время. Объяви отпечаток посреди него — и
/// клиенты перезагрузятся в `index.html`, который ссылается на ещё не
/// докопированный бандл, то есть в белый экран. Поэтому ждём, пока отпечаток
/// простоит два опроса подряд и все файлы, на которые ссылается страница,
/// окажутся на месте.
async fn watch_loop(web_root: PathBuf, tx: watch::Sender<String>) {
    let mut candidate: Option<String> = None;
    loop {
        tokio::time::sleep(POLL).await;
        let root = web_root.clone();
        let seen = tokio::task::spawn_blocking(move || fingerprint(&root))
            .await
            .ok()
            .flatten();
        let Some(seen) = seen else {
            candidate = None;
            continue;
        };
        if *tx.borrow() == seen {
            candidate = None;
            continue;
        }
        if candidate.as_deref() == Some(seen.as_str()) {
            eprintln!("Новая сборка интерфейса {seen}, оповещаю клиентов");
            tx.send_replace(seen);
            candidate = None;
        } else {
            candidate = Some(seen);
        }
    }
}

/// Отпечаток сборки: содержимое `index.html` плюс имя, размер и время
/// изменения каждого файла в `assets/`.
///
/// Одних имён файлов мало: сборщик уже выдавал одинаковое имя бандла для
/// разных сборок (см. `static_cache_headers` в main.rs).
///
/// `None` — сборка неполная: страница ссылается на файл, которого нет.
fn fingerprint(web_root: &Path) -> Option<String> {
    let html = std::fs::read_to_string(web_root.join("index.html")).ok()?;
    if !referenced_assets(&html).all(|rel| web_root.join(rel).is_file()) {
        return None;
    }
    let mut entries: Vec<(String, u64, u128)> = std::fs::read_dir(web_root.join("assets"))
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            let meta = e.metadata().ok()?;
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            Some((e.file_name().to_string_lossy().into_owned(), meta.len(), modified))
        })
        .collect();
    entries.sort();
    let mut hash = Sha256::new();
    hash.update(html.as_bytes());
    for (name, len, modified) in entries {
        hash.update(format!("\n{name}\t{len}\t{modified}").as_bytes());
    }
    Some(hex::encode(&hash.finalize()[..8]))
}

/// Пути `assets/…`, на которые ссылается страница.
fn referenced_assets(html: &str) -> impl Iterator<Item = &str> {
    html.match_indices("\"/assets/").filter_map(|(at, _)| {
        let rest = &html[at + 2..];
        rest.find('"').map(|end| &rest[..end])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meshkeeper-ui-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        dir
    }

    const PAGE: &str = "<html><head><script type=\"module\" src=\"/assets/index-A.js\"></script></head><body></body></html>";

    #[test]
    fn script_goes_into_head_with_build_mark() {
        let out = inject(PAGE, "abc123");
        let head_end = out.find("</head>").unwrap();
        let mark = out.find("window.__MK_BUILD=\"abc123\"").unwrap();
        let script = out.find("/api/app/update.js").unwrap();
        assert!(mark < script && script < head_end);
    }

    #[test]
    fn half_copied_build_is_not_announced() {
        let root = temp_root("partial");
        std::fs::write(root.join("index.html"), PAGE).unwrap();
        // Бандла, на который ссылается страница, ещё нет.
        assert_eq!(fingerprint(&root), None);
        std::fs::write(root.join("assets/index-A.js"), "1").unwrap();
        assert!(fingerprint(&root).is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn same_bundle_name_with_new_content_changes_fingerprint() {
        let root = temp_root("rename");
        std::fs::write(root.join("index.html"), PAGE).unwrap();
        std::fs::write(root.join("assets/index-A.js"), "old").unwrap();
        let before = fingerprint(&root).unwrap();
        std::fs::write(root.join("assets/index-A.js"), "new build").unwrap();
        assert_ne!(fingerprint(&root).unwrap(), before);
        let _ = std::fs::remove_dir_all(root);
    }
}
