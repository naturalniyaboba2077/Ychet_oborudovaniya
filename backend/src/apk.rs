//! Раздача новой версии Android-приложения.
//!
//! Интерфейс в приложении и так обновляется на лету (см. update.rs): APK —
//! тонкий клиент и открывает страницу с сервера. Но сама оболочка — WebView,
//! сканер QR, мост к JS — меняется только установкой нового APK. Раньше для
//! этого человеку надо было получить файл и поставить его вручную, поэтому
//! на телефонах годами жили старые оболочки.
//!
//! Теперь выкладка кладёт APK в каталог `apk/` рядом с сервисом под именем
//! `meshkeeper-<versionCode>.apk`, а приложение при запуске спрашивает
//! `/api/app/android` и, если номер больше своего, скачивает и ставит.
//!
//! Номер берём из имени файла, а не из манифеста внутри APK: манифест там
//! в двоичном XML, и разбирать его ради одного числа — лишний код и лишний
//! повод ошибиться. Подлинность проверяет сам Android: обновление с чужой
//! подписью система не установит.

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Releases {
    dir: PathBuf,
}

pub fn routes<S>(dir: PathBuf) -> Router<S> {
    Router::new()
        .route("/api/app/android", get(latest))
        .route("/api/app/android.apk", get(download))
        .with_state(Arc::new(Releases { dir }))
}

/// Последний выложенный APK: номер версии и путь.
///
/// Файлы с другими именами пропускаем — в том числе недокопированные
/// `*.apk.part`: выкладка переименовывает файл, только когда он на месте.
fn newest(dir: &Path) -> Option<(u32, PathBuf)> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            let code = name.strip_prefix("meshkeeper-")?.strip_suffix(".apk")?;
            let code: u32 = code.parse().ok()?;
            e.path().is_file().then(|| (code, e.path()))
        })
        .max_by_key(|(code, _)| *code)
}

fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hash.update(&buf[..n]);
    }
    Ok((hex::encode(hash.finalize()), size))
}

/// Описание последней версии. Контрольная сумма — чтобы приложение
/// отличило оборванную загрузку от целого файла и не предлагало
/// человеку поставить обрывок.
async fn latest(State(rel): State<Arc<Releases>>) -> Response {
    let dir = rel.dir.clone();
    let found = tokio::task::spawn_blocking(move || {
        let (code, path) = newest(&dir)?;
        let (sha256, size) = sha256_file(&path).ok()?;
        Some((code, sha256, size))
    })
    .await
    .ok()
    .flatten();
    let Some((code, sha256, size)) = found else {
        return (StatusCode::NOT_FOUND, "APK не выложен").into_response();
    };
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "versionCode": code,
            "sha256": sha256,
            "size": size,
            "url": format!("/api/app/android.apk?v={code}"),
        })),
    )
        .into_response()
}

async fn download(State(rel): State<Arc<Releases>>) -> Response {
    let Some((code, path)) = newest(&rel.dir) else {
        return (StatusCode::NOT_FOUND, "APK не выложен").into_response();
    };
    // Целиком в память: APK весит несколько мегабайт, а качают его редко —
    // раз на выход версии с каждого телефона.
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return (StatusCode::NOT_FOUND, "APK не выложен").into_response();
    };
    (
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.android.package-archive".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"meshkeeper-{code}.apk\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_picks_highest_code_and_skips_partial_uploads() {
        let dir = std::env::temp_dir().join(format!("meshkeeper-apk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(newest(&dir).is_none());
        for name in [
            "meshkeeper-9.apk",
            "meshkeeper-12.apk",
            "meshkeeper-40.apk.part",
            "meshkeeper-x.apk",
            "other-99.apk",
        ] {
            std::fs::write(dir.join(name), name).unwrap();
        }
        let (code, path) = newest(&dir).unwrap();
        assert_eq!(code, 12);
        assert!(path.ends_with("meshkeeper-12.apk"));
        let (sha, size) = sha256_file(&path).unwrap();
        assert_eq!(size, "meshkeeper-12.apk".len() as u64);
        assert_eq!(sha.len(), 64);
        let _ = std::fs::remove_dir_all(dir);
    }
}
