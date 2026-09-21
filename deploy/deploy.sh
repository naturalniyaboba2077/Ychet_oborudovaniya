#!/usr/bin/env bash
# Развёртывание центрального сервера MeshKeeper.
#
# Реквизиты в репозитории не хранятся: адрес и пользователь берутся из
# окружения, аутентификация — по SSH-ключу (пароли скрипт не спрашивает).
#
#   export MESHKEEPER_DEPLOY_HOST=203.0.113.10
#   export MESHKEEPER_DEPLOY_USER=meshkeeper
#   ./deploy/deploy.sh
#
# Перед первым запуском на сервере должны существовать:
#   /opt/meshkeeper                 — каталог сервиса
#   /var/lib/meshkeeper             — каталог базы
#   /etc/meshkeeper/meshkeeper.env  — файл с MESHKEEPER_SYNC_TOKEN (chmod 600)
# См. deploy/README.md.

set -euo pipefail

HOST="${MESHKEEPER_DEPLOY_HOST:?Задайте MESHKEEPER_DEPLOY_HOST}"
USER="${MESHKEEPER_DEPLOY_USER:?Задайте MESHKEEPER_DEPLOY_USER}"
TARGET="$USER@$HOST"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BINARY="$ROOT/dist/server/meshkeeper-node"
PUBLIC="$ROOT/dist/public"

# Адрес, который слушает узел. Наружу он не смотрит — его публикует обратный
# прокси, — но порт приходится задавать: 8080 на машине может быть уже занят
# чужим сервисом, и тогда MeshKeeper просто не поднимется.
BIND="${MESHKEEPER_BIND:-127.0.0.1:8080}"

if [[ ! -f "$BINARY" ]]; then
  echo "Нет $BINARY. Соберите Linux-бинарник:" >&2
  echo "  cargo build --release --manifest-path backend/Cargo.toml --target x86_64-unknown-linux-gnu" >&2
  exit 1
fi
if [[ ! -f "$PUBLIC/index.html" ]]; then
  echo "Нет собранного фронтенда ($PUBLIC). Выполните: npm run build" >&2
  exit 1
fi

echo "→ Проверяю доступ к $TARGET"
ssh -o BatchMode=yes "$TARGET" 'test -d /opt/meshkeeper' || {
  echo "Нет доступа по ключу или не создан /opt/meshkeeper. См. deploy/README.md" >&2
  exit 1
}

echo "→ Загружаю новую версию во временный каталог"
ssh "$TARGET" 'rm -rf /opt/meshkeeper/incoming && mkdir -p /opt/meshkeeper/incoming'
scp -q "$BINARY" "$TARGET:/opt/meshkeeper/incoming/meshkeeper-node"
scp -qr "$PUBLIC" "$TARGET:/opt/meshkeeper/incoming/public"
# Юнит подставляем с нужным адресом, а не шлём как есть.
UNIT="$(mktemp)"
trap 'rm -f "$UNIT"' EXIT
sed "s|^Environment=MESHKEEPER_BIND=.*|Environment=MESHKEEPER_BIND=$BIND|"   "$ROOT/deploy/meshkeeper.service" > "$UNIT"
grep -q "MESHKEEPER_BIND=$BIND" "$UNIT" || {
  echo "Не удалось подставить адрес $BIND в юнит" >&2
  exit 1
}
scp -q "$UNIT" "$TARGET:/opt/meshkeeper/incoming/meshkeeper.service"

echo "→ Переключаю сервис"
# Единственная команда, разрешённая деплой-пользователю через sudo. Она ставится
# скриптом deploy/bootstrap.ps1 и делает переключение целиком, поэтому в sudoers
# не нужны шаблоны с подстановками.
# База не трогается: она живёт в /var/lib/meshkeeper и переживает выкладку.
ssh "$TARGET" 'sudo /usr/local/sbin/meshkeeper-activate'

echo "→ Проверяю здоровье"
ssh "$TARGET" 'curl -fsS http://127.0.0.1:8080/health' && echo
echo "Готово."
