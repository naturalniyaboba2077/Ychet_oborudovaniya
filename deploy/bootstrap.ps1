<#
.SYNOPSIS
Готовит сервер к выкладке MeshKeeper. Запускается один раз.

.DESCRIPTION
Делает всё, для чего нужен пароль сервера, за одно подключение:
  * создаёт ключ для выкладки на этой машине (если его ещё нет);
  * заводит на сервере пользователя meshkeeper и его каталоги;
  * кладёт публичный ключ в authorized_keys;
  * генерирует общий секрет синхронизации;
  * ставит /usr/local/sbin/meshkeeper-activate и узкое правило sudo;
  * прописывает алиас хоста в ~/.ssh/config.

Пароль сервера спросит сам ssh — введёте его один раз, вручную.
Скрипт пароль не сохраняет и никуда не передаёт.

.EXAMPLE
./deploy/bootstrap.ps1 -Server 203.0.113.10
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Server,

    # Учётка с правами root для первичной настройки.
    [string]$AdminUser = 'root',

    # Имя записи в ~/.ssh/config, его потом передаём в deploy.sh.
    [string]$Alias = 'meshkeeper',

    # Имя файла ключа в ~/.ssh. Отдельный параметр нужен, потому что ключ
    # нельзя переиспользовать вслепую: если прежний куда-то утёк, на новую
    # машину он потащит за собой и утечку. Для нового сервера берите новое имя.
    [string]$KeyName = 'meshkeeper_deploy',

    # Куда положить приложение, базу и резервные копии.
    [string]$InstallDir = '/opt/meshkeeper',

    # Под кем работает сервис. Значение 'root' допустимо, но тогда дыра в
    # приложении открывает всю машину, а не один каталог: отдельный
    # пользователь для того и заведён. Root нужен, только если каталог
    # установки лежит внутри /root — туда больше никто не войдёт.
    [string]$ServiceUser = 'meshkeeper',

    # Адрес, который слушает узел. Наружу он не смотрит, но порт задавать
    # приходится: 8080 на машине может быть уже занят чужим сервисом, и тогда
    # MeshKeeper молча не поднимется.
    [string]$Bind = '127.0.0.1:8080'
)

$ErrorActionPreference = 'Stop'

$sshDir = Join-Path $env:USERPROFILE '.ssh'
$keyPath = Join-Path $sshDir $KeyName
$pubPath = "$keyPath.pub"

if (-not (Test-Path $sshDir)) {
    New-Item -ItemType Directory -Path $sshDir | Out-Null
}

# ── 1. Ключ выкладки ────────────────────────────────────────────────────────
if (Test-Path $keyPath) {
    Write-Host "Ключ уже есть: $keyPath" -ForegroundColor DarkGray
}
else {
    Write-Host 'Создаю ключ для выкладки' -ForegroundColor Cyan
    # Без парольной фразы: ключ ведёт под непривилегированного пользователя,
    # sudo у него ограничен одной командой. Иначе выкладка требовала бы
    # разблокировки агента при каждом запуске.
    ssh-keygen -t ed25519 -f $keyPath -N '""' -C 'meshkeeper deploy' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'ssh-keygen не отработал' }
}

$pubKey = (Get-Content $pubPath -Raw).Trim()

# ── 2. Скрипт настройки сервера ─────────────────────────────────────────────
# Одинарные кавычки здесь принципиальны: внутри полно $-конструкций bash,
# и любая интерполяция PowerShell выполнила бы их на этой машине.
$remoteTemplate = @'
set -euo pipefail

PUBKEY='__PUBKEY__'
INSTALL_DIR='__INSTALL_DIR__'
SERVICE_USER='__SERVICE_USER__'
ENV_FILE="$INSTALL_DIR/meshkeeper.env"

if [ "$SERVICE_USER" != root ] && ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
  adduser --disabled-password --gecos "" --home "$INSTALL_DIR" --shell /bin/bash "$SERVICE_USER"
fi

mkdir -p "$INSTALL_DIR" "$INSTALL_DIR/data" "$INSTALL_DIR/backups"
chown -R "$SERVICE_USER":"$SERVICE_USER" "$INSTALL_DIR"

# Ключ кладём в домашний каталог того, под кем будет ходить выкладка.
if [ "$SERVICE_USER" = root ]; then
  SSH_HOME=/root
else
  SSH_HOME="$INSTALL_DIR"
fi
install -d -m 700 -o "$SERVICE_USER" -g "$SERVICE_USER" "$SSH_HOME/.ssh"
touch "$SSH_HOME/.ssh/authorized_keys"
grep -qxF "$PUBKEY" "$SSH_HOME/.ssh/authorized_keys" \
  || echo "$PUBKEY" >> "$SSH_HOME/.ssh/authorized_keys"
chown "$SERVICE_USER":"$SERVICE_USER" "$SSH_HOME/.ssh/authorized_keys"
chmod 600 "$SSH_HOME/.ssh/authorized_keys"

# Общий секрет сервера и локальных узлов. Существующий не перегенерируем:
# иначе отвалятся уже настроенные узлы.
if [ ! -f "$ENV_FILE" ]; then
  printf 'MESHKEEPER_SYNC_TOKEN=%s\n' "$(openssl rand -hex 32)" > "$ENV_FILE"
fi
chown "$SERVICE_USER":"$SERVICE_USER" "$ENV_FILE"
chmod 600 "$ENV_FILE"

# ProtectHome прячет /root и /home от сервиса. Если приложение лежит внутри
# /root, эта защита сделала бы его недоступным ему самому — снимаем и
# говорим об этом вслух, чтобы потом не гадать.
if [ "${INSTALL_DIR#/root}" != "$INSTALL_DIR" ]; then
  PROTECT_HOME='# ProtectHome снят: каталог установки лежит внутри /root'
  echo 'ЗАМЕТКА: установка в /root — сервис работает от root, ProtectHome отключён'
else
  PROTECT_HOME='ProtectHome=yes'
fi

cat > /etc/systemd/system/meshkeeper.service <<UNIT
[Unit]
Description=MeshKeeper — учёт оборудования (центральный сервер)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_USER
WorkingDirectory=$INSTALL_DIR

# Секреты держим в отдельном файле с правами 600, а не в юните.
EnvironmentFile=$ENV_FILE
ExecStart=$INSTALL_DIR/meshkeeper-node

Restart=always
RestartSec=3

# Узел слушает только локальный порт; наружу его публикует обратный прокси с TLS.
Environment=MESHKEEPER_BIND=__BIND__
Environment=MESHKEEPER_DB=$INSTALL_DIR/data/meshkeeper.db
Environment=MESHKEEPER_WEB_ROOT=$INSTALL_DIR/public
Environment=MESHKEEPER_COOKIE_SECURE=1

NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
$PROTECT_HOME
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
ReadWritePaths=$INSTALL_DIR
RestrictAddressFamilies=AF_INET AF_INET6
LockPersonality=yes
MemoryDenyWriteExecute=yes

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload

# Единственная команда, которую выкладке разрешено выполнять от root.
# Юнит она не трогает: его пишет этот скрипт, чтобы выкладка не могла
# подменить настройки сервиса.
cat > /usr/local/sbin/meshkeeper-activate <<ACTIVATE
#!/usr/bin/env bash
set -euo pipefail
cd "$INSTALL_DIR"
[ -x incoming/meshkeeper-node ] || { echo 'нет incoming/meshkeeper-node' >&2; exit 1; }

systemctl stop meshkeeper 2>/dev/null || true
install -m 0755 -o "$SERVICE_USER" -g "$SERVICE_USER" incoming/meshkeeper-node ./meshkeeper-node
rm -rf ./public.old
[ -d ./public ] && mv ./public ./public.old
mv incoming/public ./public
chown -R "$SERVICE_USER":"$SERVICE_USER" ./public
rm -rf incoming ./public.old

systemctl enable meshkeeper >/dev/null 2>&1 || true
systemctl restart meshkeeper
sleep 2
systemctl is-active --quiet meshkeeper || { journalctl -u meshkeeper -n 40 --no-pager >&2; exit 1; }
echo ACTIVATE_OK
ACTIVATE
chmod 0755 /usr/local/sbin/meshkeeper-activate

if [ "$SERVICE_USER" != root ]; then
  printf '%s ALL=(root) NOPASSWD: /usr/local/sbin/meshkeeper-activate\n' "$SERVICE_USER" \
    > /etc/sudoers.d/meshkeeper
  chmod 0440 /etc/sudoers.d/meshkeeper
  visudo -c -f /etc/sudoers.d/meshkeeper >/dev/null
fi

command -v cargo >/dev/null 2>&1 || echo 'ЗАМЕТКА: cargo не установлен — Linux-бинарник придётся собирать не здесь'
command -v nginx >/dev/null 2>&1 || echo 'ЗАМЕТКА: nginx не установлен'

echo BOOTSTRAP_OK
'@

$remote = $remoteTemplate.Replace('__PUBKEY__', $pubKey).Replace('__INSTALL_DIR__', $InstallDir).Replace('__SERVICE_USER__', $ServiceUser).Replace('__BIND__', $Bind).Replace("`r`n", "`n")

# base64 избавляет от возни с экранированием кавычек между PowerShell и bash.
$bytes = [Text.Encoding]::UTF8.GetBytes($remote)
$encoded = [Convert]::ToBase64String($bytes)

Write-Host ''
Write-Host 'Подключаюсь к серверу. Сейчас ssh спросит пароль — введите его.' -ForegroundColor Cyan
Write-Host 'Это единственный раз, когда пароль понадобится.' -ForegroundColor DarkGray
Write-Host ''

$output = ssh -o StrictHostKeyChecking=accept-new "$AdminUser@$Server" "echo $encoded | base64 -d | bash"
$sshExit = $LASTEXITCODE
$output | ForEach-Object { Write-Host "   $_" }

if ($output -notcontains 'BOOTSTRAP_OK') {
    # 255 — ssh не смог установить соединение (в том числе смена ключа хоста).
    if ($sshExit -eq 255) {
        $known = Join-Path $sshDir 'known_hosts'
        $seen = $null
        if (Test-Path $known) { $seen = ssh-keygen -F $Server -f $known 2>$null }

        Write-Host ''
        if ($seen) {
            Write-Host 'ОТПЕЧАТОК СЕРВЕРА НЕ СОВПАЛ С ЗАПИСАННЫМ.' -ForegroundColor Red
            Write-Host ''
            Write-Host 'Это бывает по двум причинам:' -ForegroundColor Yellow
            Write-Host '  * сервер переустановили или IP выдали другой машине — тогда всё в порядке;'
            Write-Host '  * трафик перехватывают либо сервером завладел кто-то ещё.'
            Write-Host ''
            Write-Host 'Сверьте новый отпечаток через панель хостера (VNC/консоль):' -ForegroundColor Yellow
            Write-Host '  ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub'
            Write-Host ''
            Write-Host 'Совпал — удалите устаревшую запись и повторите:' -ForegroundColor Yellow
            Write-Host "  ssh-keygen -R $Server"
            Write-Host ''
            Write-Host 'Не совпал — не подключайтесь и разбирайтесь с хостером.' -ForegroundColor Red
        }
        else {
            Write-Host 'Не удалось подключиться к серверу.' -ForegroundColor Red
            Write-Host 'Проверьте адрес, доступность порта 22 и правильность пароля.'
        }
        Write-Host ''
    }
    throw 'Настройка сервера не завершилась. Смотрите вывод выше.'
}

# ── 3. Алиас хоста ──────────────────────────────────────────────────────────
$configPath = Join-Path $sshDir 'config'
$identity = $keyPath -replace '\\', '/'
$entry = @"

Host $Alias
    HostName $Server
    User $ServiceUser
    IdentityFile $identity
    IdentitiesOnly yes
"@

$existing = if (Test-Path $configPath) { Get-Content $configPath -Raw } else { '' }
if ($existing -match "(?m)^Host\s+$([regex]::Escape($Alias))\s*$") {
    # Запись под этим именем уже была — почти наверняка от прежнего сервера.
    # Оставить её нельзя: алиас указывал бы на старую машину, и выкладка
    # молча уходила бы не туда. Заменяем целиком.
    Write-Host "Запись Host $Alias уже есть — обновляю на $Server" -ForegroundColor Yellow
    $pattern = "(?ms)^Host\s+$([regex]::Escape($Alias))\s*$.*?(?=^Host\s|\z)"
    $cleaned = [regex]::Replace($existing, $pattern, '')
    Set-Content -Path $configPath -Value $cleaned.TrimEnd() -Encoding ascii
    Add-Content -Path $configPath -Value $entry -Encoding ascii
}
else {
    # ascii, а не utf8: PowerShell 5.1 добавил бы BOM, и ssh не понял бы файл.
    Add-Content -Path $configPath -Value $entry -Encoding ascii
    Write-Host "Добавил Host $Alias в ~/.ssh/config" -ForegroundColor Cyan
}

# ── 4. Проверка входа по ключу ──────────────────────────────────────────────
Write-Host ''
Write-Host 'Проверяю вход по ключу (пароль спрашивать не должен)' -ForegroundColor Cyan
# -n обязателен: без него ssh забирает консольный ввод и ждёт его
# бесконечно — скрипт выглядит зависшим, хотя доступ уже настроен.
$probe = ssh -n -o BatchMode=yes -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new $Alias 'echo KEY_OK'
if ($probe -notcontains 'KEY_OK') {
    throw "Вход по ключу не работает. Проверьте: ssh -v $Alias"
}

Write-Host ''
Write-Host 'Готово. Вход по ключу работает.' -ForegroundColor Green
Write-Host ''
Write-Host 'Дальше:' -ForegroundColor Green
Write-Host "  1. Напишите ассистенту: сервер готов, алиас $Alias"
Write-Host '  2. Смените root-пароль сервера — старый скомпрометирован.'
Write-Host '  3. Для HTTPS нужен домен, направленный на этот сервер:'
Write-Host '     без TLS вход в приложение работать не будет (cookie Secure).'
