//! Вход через Google — OAuth 2.0 authorization code + OpenID Connect.
//!
//! Код обменивается на токены с сервера, напрямую к accounts.google.com по
//! TLS. Поэтому подпись `id_token` отдельно не проверяется: подлинность даёт
//! сам канал, и Google для серверного сценария разрешает пропустить проверку.
//! Если поток когда-нибудь переедет на клиент (implicit, One Tap), проверку
//! подписи придётся добавить — там токен приходит через браузер, а ему верить
//! нельзя.
//!
//! Незавершённые попытки входа лежат в `google_pending`: браузер уходит на
//! Google и возвращается уже другим запросом, поэтому телефон, имя и код
//! приглашения надо где-то передержать. Ключ `state` заодно защищает от
//! подделки ответа — Google возвращает его неизменным.

use anyhow::{anyhow, Result};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Сколько живёт незавершённая попытка входа. Человеку хватает и минуты,
/// но у Google бывает экран согласия и выбор аккаунта.
const PENDING_TTL_SECS: i64 = 900;

fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn client_id() -> Option<String> {
    env_value("MESHKEEPER_GOOGLE_CLIENT_ID")
}

pub fn client_secret() -> Option<String> {
    env_value("MESHKEEPER_GOOGLE_SECRET")
}

/// Должен совпадать с тем, что вписан в Google Cloud Console, до символа.
pub fn redirect_uri() -> Option<String> {
    env_value("MESHKEEPER_GOOGLE_REDIRECT")
}

/// Вход через Google предлагается, только когда настроено всё трое.
/// Иначе кнопка привела бы человека на ошибку Google.
pub fn enabled() -> bool {
    client_id().is_some() && client_secret().is_some() && redirect_uri().is_some()
}

/// Кто вошёл, по данным Google.
pub struct Identity {
    /// Неизменный идентификатор аккаунта. Именно он хранится в базе:
    /// почту человек может сменить, `sub` — нет.
    pub sub: String,
    pub email: String,
    pub name: String,
}

/// Что человек ввёл до ухода на Google.
pub struct Pending {
    pub invite_token: Option<String>,
    pub phone: Option<String>,
    pub full_name: Option<String>,
    /// Кто просил привязать Google к своей карточке. Заполняется, только
    /// когда запрос пришёл с живой сессией, — это и есть доказательство прав.
    pub link_user_id: Option<i64>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Проценты-кодирование для строки запроса. Отдельной зависимости ради
/// четырёх параметров брать не стали.
fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn random_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

/// Заводит попытку входа и возвращает адрес, куда отправить браузер.
pub fn begin(
    conn: &Connection,
    invite_token: Option<&str>,
    phone: Option<&str>,
    full_name: Option<&str>,
    link_user_id: Option<i64>,
) -> Result<String> {
    let client_id = client_id().ok_or_else(|| anyhow!("Вход через Google не настроен"))?;
    let redirect = redirect_uri().ok_or_else(|| anyhow!("Вход через Google не настроен"))?;
    let state = random_state();
    conn.execute(
        "INSERT INTO google_pending (state, invite_token, phone, full_name, link_user_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![state, invite_token, phone, full_name, link_user_id, now()],
    )?;
    prune(conn);
    Ok(format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&prompt=select_account",
        urlencode(&client_id),
        urlencode(&redirect),
        urlencode("openid email profile"),
        urlencode(&state),
    ))
}

/// Забирает попытку по `state`. Одноразово: повторный ответ с тем же `state`
/// уже ничего не найдёт, поэтому переигрывать перехваченный ответ бесполезно.
pub fn take_pending(conn: &Connection, state: &str) -> Option<Pending> {
    let row = conn
        .query_row(
            "SELECT invite_token, phone, full_name, link_user_id, created_at FROM google_pending WHERE state=?1",
            params![state],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()?;
    let _ = conn.execute("DELETE FROM google_pending WHERE state=?1", params![state]);
    let created = chrono::DateTime::parse_from_rfc3339(&row.4).ok()?;
    if (chrono::Utc::now() - created.with_timezone(&chrono::Utc)).num_seconds() > PENDING_TTL_SECS {
        return None;
    }
    Some(Pending {
        invite_token: row.0,
        phone: row.1,
        full_name: row.2,
        link_user_id: row.3,
    })
}

/// Чистит брошенные попытки — человек мог закрыть вкладку на экране Google.
fn prune(conn: &Connection) {
    let edge = chrono::Utc::now() - chrono::Duration::seconds(PENDING_TTL_SECS);
    let _ = conn.execute(
        "DELETE FROM google_pending WHERE created_at < ?1",
        params![edge.to_rfc3339()],
    );
}

/// Меняет код на токены и достаёт из `id_token` кто это.
pub async fn exchange(code: &str) -> Result<Identity> {
    let client_id = client_id().ok_or_else(|| anyhow!("Вход через Google не настроен"))?;
    let secret = client_secret().ok_or_else(|| anyhow!("Вход через Google не настроен"))?;
    let redirect = redirect_uri().ok_or_else(|| anyhow!("Вход через Google не настроен"))?;
    let response = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&[
            ("code", code),
            ("client_id", client_id.as_str()),
            ("client_secret", secret.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Тело ответа Google несёт разбор ошибки (redirect_uri_mismatch и
        // подобное) — без него настройку не починить.
        return Err(anyhow!("Google отказал ({status}): {body}"));
    }
    let body: Value = response.json().await?;
    let id_token = body
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Google не вернул id_token"))?;
    identity_from_id_token(id_token)
}

/// Разбирает полезную нагрузку `id_token`. Подпись не проверяется намеренно —
/// см. пояснение в шапке модуля.
pub fn identity_from_id_token(id_token: &str) -> Result<Identity> {
    use base64::Engine;
    let payload = id_token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow!("id_token без полезной нагрузки"))?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload)?;
    let claims: Value = serde_json::from_slice(&raw)?;
    let sub = claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("id_token без sub"))?;
    // Незаверенную почту принимать нельзя: иначе чужой аккаунт с такой же
    // почтой мог бы прицепиться к существующему пользователю.
    let verified = claims
        .get("email_verified")
        .map(|v| v == &Value::Bool(true) || v == &Value::String("true".into()))
        .unwrap_or(false);
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !email.is_empty() && !verified {
        return Err(anyhow!("Google не подтвердил эту почту"));
    }
    let name = claims
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(Identity {
        sub: sub.to_string(),
        email: email.to_string(),
        name: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn token(claims: Value) -> String {
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("header.{payload}.signature")
    }

    #[test]
    fn reads_identity_from_payload() {
        let t = token(serde_json::json!({
            "sub": "10769150350006150715113082367",
            "email": "brigadir@example.com",
            "email_verified": true,
            "name": "Пётр Кузнецов",
        }));
        let id = identity_from_id_token(&t).expect("разбор");
        assert_eq!(id.sub, "10769150350006150715113082367");
        assert_eq!(id.email, "brigadir@example.com");
        assert_eq!(id.name, "Пётр Кузнецов");
    }

    #[test]
    fn rejects_unverified_email() {
        let t = token(serde_json::json!({
            "sub": "42",
            "email": "chuzhoy@example.com",
            "email_verified": false,
        }));
        assert!(identity_from_id_token(&t).is_err());
    }

    #[test]
    fn rejects_token_without_sub() {
        let t = token(serde_json::json!({"email": "a@b.c", "email_verified": true}));
        assert!(identity_from_id_token(&t).is_err());
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(
            urlencode("openid email profile"),
            "openid%20email%20profile"
        );
        assert_eq!(
            urlencode("https://a.example/cb?x=1"),
            "https%3A%2F%2Fa.example%2Fcb%3Fx%3D1"
        );
    }
}
