//! Вход, регистрация и приглашения.
//!
//! Отделено от `api/mod.rs`: там остались общие помощники,
//! проверка прав и диспетчер.

use super::*;

pub(crate) fn auth_directory(conn: &Connection) -> ApiResult {
    let mut stmt = conn.prepare("SELECT id FROM users WHERE status!='disabled' ORDER BY id")?;
    let ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    let mut out = Vec::new();
    for id in ids {
        if let Some(mut u) = jsn::user_public(conn, id) {
            let has: Option<String> = conn
                .query_row(
                    "SELECT password_hash FROM users WHERE id=?1",
                    params![id],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            u["hasPassword"] = json!(has.filter(|s| !s.is_empty()).is_some());
            out.push(u);
        }
    }
    Ok(Value::Array(out))
}

/// Что экран входа может предложить прямо сейчас. Регистрация владельца
/// доступна, пока база пуста (bootstrap) либо если она открыта явно.
pub(crate) fn auth_options(conn: &Connection) -> ApiResult {
    let users: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .unwrap_or(0);
    Ok(json!({
        // Аккаунт заводит любой: организация создаётся отдельным шагом, и
        // пока её нет, учётная запись пустая — смотреть в ней нечего.
        "registrationOpen": true,
        "bootstrap": users == 0,
        "demoLogin": std::env::var("MESHKEEPER_DEMO_LOGIN").as_deref() == Ok("1"),
        "googleEnabled": crate::google::enabled(),
    }))
}

/// Готовит вход через Google и возвращает адрес, куда уходит браузер.
///
/// Телефон и имя принимаются здесь, а не после возвращения от Google:
/// номер в системе обязателен, а Google его не сообщает.
pub(crate) fn auth_google_begin(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    if !crate::google::enabled() {
        return Err(ApiError::bad("Вход через Google не настроен"));
    }
    let invite = s(input, "inviteToken");
    let phone = s(input, "phone");
    let full_name = s(input, "fullName");
    // Приглашение проверяем сразу: незачем гонять человека на Google, чтобы
    // отказать ему на обратном пути.
    if let Some(token) = invite.as_deref() {
        let found = invite_by_token(conn, token)?;
        ensure_invite_usable(&found)?;
        if phone.is_none() {
            return Err(ApiError::bad("Введите телефон"));
        }
    }
    let url = crate::google::begin(
        conn,
        invite.as_deref(),
        phone.as_deref(),
        full_name.as_deref(),
        user_id,
    )
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(json!({ "url": url }))
}

/// Завершает вход: находит или заводит пользователя по ответу Google.
/// Возвращает его идентификатор — сессию выдаёт вызывающий.
pub fn google_finish(
    conn: &Connection,
    identity: &crate::google::Identity,
    pending: &crate::google::Pending,
) -> Result<i64, ApiError> {
    // Уже привязанный аккаунт — просто вход.
    if let Some(uid) = conn
        .query_row(
            "SELECT id FROM users WHERE google_sub=?1",
            params![identity.sub],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        if let Some(token) = pending.invite_token.as_deref() {
            consume_invite(conn, token, uid)?;
        }
        return Ok(uid);
    }
    // Привязка к уже вошедшему — он доказал права сессией, так что просто
    // дописываем Google к его карточке.
    if let Some(link_to) = pending.link_user_id {
        conn.execute(
            "UPDATE users SET google_sub=?1, email=COALESCE(NULLIF(email,''),?2) WHERE id=?3",
            params![identity.sub, identity.email, link_to],
        )?;
        if let Some(token) = pending.invite_token.as_deref() {
            consume_invite(conn, token, link_to)?;
        }
        return Ok(link_to);
    }
    // Новый человек без приглашения. Аккаунт завести можно — организация
    // создаётся отдельным шагом, и пока её нет, смотреть в учётной записи
    // нечего. Телефон всё равно нужен: в системе учёта именно он связывает
    // карточку с живым человеком, а Google его не сообщает, поэтому форма
    // спрашивает номер до перехода.
    let Some(token) = pending.invite_token.as_deref() else {
        let phone = pending
            .phone
            .as_deref()
            .ok_or_else(|| ApiError::bad("Укажите телефон — без него учётную запись не завести"))?;
        if find_user_phone(conn, phone).is_some() {
            return Err(ApiError::conflict(
                "Этот телефон уже зарегистрирован. Войдите с ним и привяжите Google в профиле.",
            ));
        }
        let full_name = pending
            .full_name
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(identity.name.as_str());
        if full_name.trim().is_empty() {
            return Err(ApiError::bad("Введите имя"));
        }
        conn.execute(
            "INSERT INTO users (full_name, phone, status, role_rights, email, google_sub, created_at)
             VALUES (?1,?2,'active',?3,?4,?5,?6)",
            params![
                full_name,
                phone,
                db::default_rights().to_string(),
                identity.email,
                identity.sub,
                now()
            ],
        )
        .map_err(|e| ApiError::conflict(e.to_string()))?;
        return Ok(conn.last_insert_rowid());
    };
    let invite = invite_by_token(conn, token)?;
    ensure_invite_usable(&invite)?;
    // Телефон занят. Молча привязать сюда Google нельзя: приглашение есть у
    // любого, кому его переслали, и он вписал бы чужой номер — например
    // владельца — и забрал бы аккаунт. Пускаем только в карточку, которую
    // администратор сам завёл под этого человека и которая ждёт активации
    // именно в этой группе.
    if let Some(phone) = pending.phone.as_deref() {
        if let Some(existing) = find_user_phone(conn, phone) {
            let awaiting: i64 = conn.query_row(
                "SELECT COUNT(*) FROM user_workspaces uw JOIN users u ON u.id=uw.user_id
                 WHERE uw.user_id=?1 AND uw.workspace_id=?2 AND u.status='invited'",
                params![existing, invite.workspace_id],
                |r| r.get(0),
            )?;
            if awaiting == 0 {
                return Err(ApiError::unauth(
                    "Аккаунт с таким телефоном уже есть. Войдите паролем и привяжите Google в профиле.",
                ));
            }
            conn.execute(
                "UPDATE users SET google_sub=?1, email=COALESCE(NULLIF(email,''),?2), status='active' WHERE id=?3",
                params![identity.sub, identity.email, existing],
            )?;
            consume_invite(conn, token, existing)?;
            return Ok(existing);
        }
    }
    let phone = pending
        .phone
        .as_deref()
        .ok_or_else(|| ApiError::bad("Введите телефон"))?;
    let full_name = pending
        .full_name
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(identity.name.as_str());
    if full_name.trim().is_empty() {
        return Err(ApiError::bad("Введите имя"));
    }
    // password_hash остаётся пустым: пароля у такого аккаунта нет вовсе,
    // и auth.login его не пустит — вход только через Google.
    conn.execute(
        "INSERT INTO users (full_name, position, phone, status, role_rights, email, google_sub, created_at)
         VALUES (?1,?2,?3,'active',?4,?5,?6,?7)",
        params![
            full_name,
            invite_position(&invite.role),
            phone,
            db::rights_for_role(&invite.role).to_string(),
            identity.email,
            identity.sub,
            now()
        ],
    )
    .map_err(|e| ApiError::bad(e.to_string()))?;
    let uid = conn.last_insert_rowid();
    consume_invite(conn, token, uid)?;
    Ok(uid)
}

/// Сколько неудачных попыток проходит без задержки.
pub(crate) const LOGIN_FREE_ATTEMPTS: i64 = 5;

/// Базовая пауза после исчерпания попыток; дальше удваивается.
pub(crate) const LOGIN_LOCK_BASE_SECS: i64 = 30;

pub(crate) const LOGIN_LOCK_MAX_SECS: i64 = 900;

/// Через столько тишины счётчик неудач обнуляется.
pub(crate) const LOGIN_FAILURE_TTL_SECS: i64 = 3600;

/// Ключ троттлинга — только цифры номера, чтобы «+7 900…» и «8900…»
/// считались одной учётной записью.
pub(crate) fn throttle_key(phone: &str) -> String {
    db::digits_only(phone)
}

/// Отказ, если по этому номеру уже перебирали пароль.
pub(crate) fn check_login_allowed(conn: &Connection, key: &str) -> Result<(), ApiError> {
    let locked: Option<String> = conn
        .query_row(
            "SELECT locked_until FROM login_throttle WHERE key=?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(raw) = locked else { return Ok(()) };
    let Ok(until) = chrono::DateTime::parse_from_rfc3339(&raw) else {
        return Ok(());
    };
    let left = (until.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
    if left <= 0 {
        return Ok(());
    }
    Err(ApiError::new(
        "TOO_MANY_REQUESTS",
        429,
        format!("Слишком много попыток входа. Повторите через {left} с."),
    ))
}

pub(crate) fn note_login_failure(conn: &Connection, key: &str) {
    let now_ts = chrono::Utc::now();
    let previous: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT failures, last_failure_at FROM login_throttle WHERE key=?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    // Давние неудачи не должны копиться месяцами.
    let stale = previous
        .as_ref()
        .and_then(|(_, at)| at.as_deref())
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_none_or(|at| {
            (now_ts - at.with_timezone(&chrono::Utc)).num_seconds() > LOGIN_FAILURE_TTL_SECS
        });
    let failures = if stale {
        1
    } else {
        previous.map(|(n, _)| n).unwrap_or(0) + 1
    };

    let locked_until = if failures >= LOGIN_FREE_ATTEMPTS {
        let steps = (failures - LOGIN_FREE_ATTEMPTS).min(20) as u32;
        let secs = LOGIN_LOCK_BASE_SECS
            .saturating_mul(1_i64 << steps.min(10))
            .min(LOGIN_LOCK_MAX_SECS);
        Some((now_ts + chrono::Duration::seconds(secs)).to_rfc3339())
    } else {
        None
    };
    let _ = conn.execute(
        "INSERT INTO login_throttle (key, failures, last_failure_at, locked_until)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(key) DO UPDATE SET
           failures=excluded.failures,
           last_failure_at=excluded.last_failure_at,
           locked_until=excluded.locked_until",
        params![key, failures, now_ts.to_rfc3339(), locked_until],
    );
}

pub(crate) fn clear_login_failures(conn: &Connection, key: &str) {
    let _ = conn.execute("DELETE FROM login_throttle WHERE key=?1", params![key]);
}

pub(crate) fn auth_login(conn: &Connection, input: &Value) -> ApiResult {
    let id = if let Some(uid) = i64v(input, "userId") {
        if std::env::var("MESHKEEPER_DEMO_LOGIN").as_deref() != Ok("1") {
            return Err(ApiError::unauth("Вход по идентификатору отключён"));
        }
        uid
    } else if let Some(phone) = s(input, "phone") {
        // Проверяем до обращения к базе паролей: перебор не должен
        // получать даже ответ «есть такой аккаунт или нет».
        check_login_allowed(conn, &throttle_key(&phone))?;
        find_user_phone(conn, &phone).ok_or_else(|| {
            note_login_failure(conn, &throttle_key(&phone));
            ApiError::unauth(
                "Аккаунт не найден. Зарегистрируйтесь или отсканируйте QR-приглашение.",
            )
        })?
    } else {
        return Err(ApiError::unauth("Укажите телефон"));
    };
    let (status, hash, name): (String, Option<String>, String) = conn
        .query_row(
            "SELECT status, password_hash, full_name FROM users WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| ApiError::unauth("Аккаунт не найден"))?;
    if status == "disabled" {
        return Err(ApiError::unauth("Аккаунт заблокирован"));
    }
    let key = s(input, "phone").map(|p| throttle_key(&p));
    if let Some(h) = hash.filter(|x| !x.is_empty()) {
        let pw = s(input, "password").unwrap_or_default();
        if !verify_password(&pw, &h) {
            if let Some(k) = &key {
                note_login_failure(conn, k);
            }
            return Err(ApiError::unauth("Неверный телефон или пароль"));
        }
        if !h.starts_with("$argon2") {
            conn.execute(
                "UPDATE users SET password_hash=?1 WHERE id=?2",
                params![hash_password(&pw), id],
            )?;
        }
    } else if std::env::var("MESHKEEPER_DEMO_LOGIN").as_deref() != Ok("1") {
        return Err(ApiError::unauth("Для аккаунта ещё не установлен пароль"));
    }
    if status == "invited" {
        let _ = conn.execute("UPDATE users SET status='active' WHERE id=?1", params![id]);
    }
    if let Some(k) = &key {
        clear_login_failures(conn, k);
    }
    let mut u = jsn::user_public(conn, id).ok_or_else(|| ApiError::unauth("Аккаунт не найден"))?;
    u["fullName"] = json!(name);
    Ok(u)
}

/// Базовые статусы и склад нового пространства. Без них у предметов не будет
/// ни «В работе», ни «На проверке» (ТЗ §9), а конфликты синхронизации не смогут
/// пометить предмет как требующий проверки.
pub(crate) fn seed_workspace_defaults(
    conn: &Connection,
    ws: i64,
    owner: i64,
) -> Result<(), ApiError> {
    for (name, slug, color, bg) in [
        ("В работе", "in-work", "#2E9E5B", "#C8FCD2"),
        ("В ремонте", "in-repair", "#A87C0F", "#FBFCC8"),
        ("На складе", "in-stock", "#5E629B", "#EDEDF7"),
        ("На проверке", "needs-check", "#A87C0F", "#FBFCC8"),
        ("Списан", "written-off", "#D64545", "#FAD8D1"),
    ] {
        conn.execute(
            "INSERT INTO statuses (name, workspace_id, type, slug, color, bg)
             SELECT ?2, ?1, 'status', ?3, ?4, ?5
             WHERE NOT EXISTS (SELECT 1 FROM statuses WHERE workspace_id=?1 AND slug=?3)",
            params![ws, name, slug, color, bg],
        )?;
    }
    conn.execute(
        "INSERT INTO storages (name, responsible_user_id, workspace_id, address)
         SELECT 'Основной склад', ?2, ?1, ''
         WHERE NOT EXISTS (SELECT 1 FROM storages WHERE workspace_id=?1)",
        params![ws, owner],
    )?;
    Ok(())
}

/// Заводит учётную запись — и только её.
///
/// Организация создаётся отдельным шагом: человек сначала получает аккаунт,
/// а потом решает, завести свою группу или вступить в чужую по приглашению.
/// Раньше эти два действия были склеены, и войти через Google было нельзя в
/// принципе: Google не сообщает ни телефона, ни названия организации.
/// Сколько учётных записей разрешено завести с одного адреса за час.
///
/// Регистрация открыта: аккаунт без организации пустой, и это осознанное
/// решение. Но открытая дверь без счётчика — приглашение набить базу
/// мусором, а вместе с ней и диск, раз вложения теперь ложатся файлами.
const SIGNUPS_PER_HOUR: i64 = 5;
const SIGNUP_WINDOW_SECS: i64 = 3600;

/// Не даёт штамповать аккаунты пачками с одного адреса.
///
/// Адрес берётся из заголовка, который проставляет обратный прокси. Это не
/// защита от упорного злоумышленника — адрес меняется, — а заслон от
/// простого перебора и случайного цикла в чужом скрипте.
fn check_signup_allowed(conn: &Connection, from: &str) -> Result<(), ApiError> {
    let key = format!("signup:{from}");
    let edge = (chrono::Utc::now() - chrono::Duration::seconds(SIGNUP_WINDOW_SECS)).to_rfc3339();
    let recent: i64 = conn
        .query_row(
            "SELECT failures FROM login_throttle WHERE key=?1 AND last_failure_at > ?2",
            params![key, edge],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if recent >= SIGNUPS_PER_HOUR {
        return Err(ApiError::new(
            "TOO_MANY_REQUESTS",
            429,
            "Слишком много регистраций с этого адреса. Попробуйте позже.",
        ));
    }
    conn.execute(
        "INSERT INTO login_throttle (key, failures, last_failure_at) VALUES (?1,1,?2)
         ON CONFLICT(key) DO UPDATE SET
            failures = CASE WHEN login_throttle.last_failure_at > ?3 THEN failures + 1 ELSE 1 END,
            last_failure_at = ?2",
        params![key, chrono::Utc::now().to_rfc3339(), edge],
    )?;
    Ok(())
}

pub(crate) fn auth_register(conn: &Connection, input: &Value) -> ApiResult {
    check_signup_allowed(conn, &client_address())?;
    let full_name = s(input, "fullName").ok_or_else(|| ApiError::bad("Введите имя"))?;
    let phone = s(input, "phone").ok_or_else(|| ApiError::bad("Введите телефон"))?;
    let password = s(input, "password").ok_or_else(|| ApiError::bad("Введите пароль"))?;
    if password.chars().count() < 10 {
        return Err(ApiError::bad("Пароль минимум 10 символов"));
    }
    if find_user_phone(conn, &phone).is_some() {
        return Err(ApiError::conflict(
            "Этот телефон уже зарегистрирован. Войдите с тем же номером и паролем.",
        ));
    }
    // Прав пока никаких: они появятся вместе с группой — своей или чужой.
    conn.execute(
        "INSERT INTO users (full_name, phone, status, password_hash, role_rights, created_at)
         VALUES (?1,?2,'active',?3,?4,?5)",
        params![
            full_name,
            phone,
            hash_password(&password),
            db::default_rights().to_string(),
            now()
        ],
    )
    .map_err(|e| ApiError::conflict(e.to_string()))?;
    let uid = conn.last_insert_rowid();
    Ok(jsn::user_public(conn, uid).unwrap())
}

/// Создаёт организацию для уже вошедшего человека и делает его владельцем.
///
/// Отдельная процедура, а не admin.workspaces.create: у того требуется право
/// manageWorkspaces, которого у новичка неоткуда взяться — он ещё никуда не
/// входит.
pub(crate) fn auth_create_workspace(
    conn: &Connection,
    input: &Value,
    user_id: Option<i64>,
) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let name = s(input, "name")
        .map(|v| v.trim().to_string())
        .filter(|v| v.chars().count() >= 2)
        .ok_or_else(|| ApiError::bad("Введите название организации"))?;
    let sync_url = s(input, "syncUrl");
    conn.execute(
        "INSERT INTO workspaces (name, timezone, internal_id_prefix, comment, created_at, sync_url)
         VALUES (?1,?2,'ВН-',?3,?4,?5)",
        params![
            name,
            s(input, "timezone").unwrap_or_else(|| "Europe/Moscow".into()),
            "Создано владельцем",
            now(),
            sync_url
        ],
    )
    .map_err(|e| ApiError::bad(e.to_string()))?;
    let ws = conn.last_insert_rowid();
    if let Some(url) = sync_url {
        crate::sync::add_peer(conn, &url, Some("relay"), None);
    }
    // Владелец своей группы получает полные права в ней. Права хранятся в
    // членстве, а не в человеке: в чужой группе он может быть кем угодно.
    conn.execute(
        "INSERT INTO user_workspaces (user_id, workspace_id, rights_json) VALUES (?1,?2,?3)",
        params![uid, ws, db::owner_rights().to_string()],
    )?;
    // Глобальные права трогаем только у новичка, заводящего первую группу.
    // В одном аккаунте можно состоять в нескольких организациях с разными
    // ролями: владелец своей и простой работник в чужой. Если переписывать
    // общее поле при каждом создании, роль из одной группы протекала бы в
    // другую. Роль живёт в членстве — там её и выставили строкой выше.
    let first_group: i64 = conn.query_row(
        "SELECT COUNT(*) FROM user_workspaces WHERE user_id=?1",
        params![uid],
        |r| r.get(0),
    )?;
    if first_group <= 1 {
        conn.execute(
            "UPDATE users SET position=COALESCE(NULLIF(position,''),'Владелец'), role_rights=?1 WHERE id=?2",
            params![db::owner_rights().to_string(), uid],
        )?;
    }
    seed_workspace_defaults(conn, ws, uid)?;
    let mut me = jsn::user_public(conn, uid).unwrap();
    me["workspaceId"] = json!(ws);
    me["joinedWorkspace"] = jsn::workspace_json(conn, ws).unwrap_or(json!({}));
    Ok(me)
}

pub(crate) struct Invite {
    pub(crate) id: i64,
    pub(crate) workspace_id: i64,
    pub(crate) role: String,
    pub(crate) max_uses: i64,
    pub(crate) used_count: i64,
    pub(crate) revoked: i64,
    pub(crate) expires_at: Option<String>,
}

impl Invite {
    pub(crate) fn is_expired(&self) -> bool {
        let Some(raw) = self.expires_at.as_deref().filter(|s| !s.is_empty()) else {
            return false;
        };
        match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(ts) => ts.with_timezone(&chrono::Utc) <= chrono::Utc::now(),
            // Нечитаемый срок считаем истёкшим: приглашение не должно «оживать» из-за битой даты.
            Err(_) => true,
        }
    }
}

pub(crate) fn invite_by_token(conn: &Connection, token: &str) -> Result<Invite, ApiError> {
    conn.query_row(
        "SELECT id, workspace_id, role, max_uses, used_count, revoked, expires_at FROM invites WHERE token=?1",
        params![token],
        |r| {
            Ok(Invite {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                role: r.get(2)?,
                max_uses: r.get(3)?,
                used_count: r.get(4)?,
                revoked: r.get(5)?,
                expires_at: r.get(6)?,
            })
        },
    )
    .map_err(|_| ApiError::not_found("Приглашение недействительно или истекло"))
}

/// Общая проверка пригодности приглашения: отзыв, исчерпание и срок действия.
pub(crate) fn ensure_invite_usable(invite: &Invite) -> Result<(), ApiError> {
    if invite.revoked != 0 {
        return Err(ApiError::bad("Приглашение отозвано"));
    }
    if invite.used_count >= invite.max_uses {
        return Err(ApiError::bad("Приглашение уже использовано"));
    }
    if invite.is_expired() {
        return Err(ApiError::bad("Срок действия приглашения истёк"));
    }
    Ok(())
}

pub(crate) fn consume_invite(conn: &Connection, token: &str, user_id: i64) -> ApiResult {
    let invite = invite_by_token(conn, token)?;
    ensure_invite_usable(&invite)?;
    let (id, ws) = (invite.id, invite.workspace_id);
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM user_workspaces WHERE user_id=?1 AND workspace_id=?2",
        params![user_id, ws],
        |r| r.get(0),
    )?;
    if exists == 0 {
        conn.execute(
            "INSERT INTO user_workspaces (user_id, workspace_id, rights_json) VALUES (?1,?2,?3)",
            params![user_id, ws, db::rights_for_role(&invite.role).to_string()],
        )?;
        // Должность подписывает человека в списках. Вступление существующим
        // аккаунтом её не проставляло — в отличие от вступления с
        // регистрацией, — и человек значился без роли. Уже заполненную не
        // трогаем: её мог задать администратор.
        conn.execute(
            "UPDATE users SET position=?1 WHERE id=?2 AND (position IS NULL OR position='')",
            params![invite_position(&invite.role), user_id],
        )?;
    }
    conn.execute(
        "UPDATE invites SET used_count=used_count+1 WHERE id=?1",
        params![id],
    )?;
    Ok(jsn::workspace_json(conn, ws).unwrap_or(json!({"id": ws})))
}

pub(crate) fn auth_join(conn: &Connection, input: &Value, user_id: Option<i64>) -> ApiResult {
    let uid = require_user(conn, user_id)?;
    let token = s(input, "token").ok_or_else(|| ApiError::bad("Нет токена приглашения"))?;
    consume_invite(conn, &token, uid)
}

pub(crate) fn auth_join_register(conn: &Connection, input: &Value) -> ApiResult {
    let token = s(input, "token").ok_or_else(|| ApiError::bad("Нет токена приглашения"))?;
    let invite = invite_by_token(conn, &token)?;
    ensure_invite_usable(&invite)?;
    let ws = invite.workspace_id;
    let full_name = s(input, "fullName").ok_or_else(|| ApiError::bad("Введите имя"))?;
    let phone = s(input, "phone").ok_or_else(|| ApiError::bad("Введите телефон"))?;
    let password = s(input, "password").unwrap_or_default();
    let uid = if let Some(existing) = find_user_phone(conn, &phone) {
        let h: Option<String> = conn.query_row(
            "SELECT password_hash FROM users WHERE id=?1",
            params![existing],
            |r| r.get(0),
        )?;
        let h = h.unwrap_or_default();
        if !h.is_empty() && !verify_password(&password, &h) {
            return Err(ApiError::unauth("Неверный пароль для этого телефона"));
        }
        if h.is_empty() {
            let invited_here: i64 = conn.query_row(
                "SELECT COUNT(*) FROM user_workspaces uw JOIN users u ON u.id=uw.user_id WHERE uw.user_id=?1 AND uw.workspace_id=?2 AND u.status='invited'",
                params![existing, ws], |r| r.get(0),
            )?;
            if invited_here == 0 || password.chars().count() < 10 {
                return Err(ApiError::unauth("Аккаунт требует персональной активации"));
            }
            conn.execute(
                "UPDATE users SET password_hash=?1,status='active' WHERE id=?2",
                params![hash_password(&password), existing],
            )?;
        }
        existing
    } else {
        if password.chars().count() < 10 {
            return Err(ApiError::bad("Пароль минимум 10 символов"));
        }
        conn.execute(
            "INSERT INTO users (full_name, position, phone, status, password_hash, role_rights, created_at)
             VALUES (?1,?2,?3,'active',?4,?5,?6)",
            params![
                full_name,
                invite_position(&invite.role),
                phone,
                hash_password(&password),
                db::rights_for_role(&invite.role).to_string(),
                now()
            ],
        ).map_err(|e| ApiError::bad(e.to_string()))?;
        conn.last_insert_rowid()
    };
    let wsj = consume_invite(conn, &token, uid)?;
    let mut u = jsn::user_public(conn, uid).unwrap();
    u["joinedWorkspace"] = wsj;
    u["workspaceId"] = json!(ws);
    Ok(u)
}

pub(crate) fn invite_info(conn: &Connection, input: &Value) -> ApiResult {
    let token = s(input, "token").ok_or_else(|| ApiError::bad("token"))?;
    let invite = invite_by_token(conn, &token)?;
    ensure_invite_usable(&invite)?;
    let wsj = jsn::workspace_json(conn, invite.workspace_id).unwrap_or(json!({}));
    Ok(json!({
        "workspace": wsj,
        "role": invite.role,
        "token": token,
        "expiresAt": invite.expires_at,
    }))
}
