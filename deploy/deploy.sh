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
#   $MESHKEEPER_DEPLOY_DIR              — каталог сервиса (по умолчанию /root/meshkeeper)
#   $MESHKEEPER_DEPLOY_DIR/data         — каталог базы; выкладка его не трогает
#   $MESHKEEPER_DEPLOY_DIR/meshkeeper.env — файл с секретами (chmod 600)
#   /usr/local/sbin/meshkeeper-activate — скрипт переключения версии
# См. deploy/README.md.

set -euo pipefail

HOST="${MESHKEEPER_DEPLOY_HOST:?Задайте MESHKEEPER_DEPLOY_HOST}"
USER="${MESHKEEPER_DEPLOY_USER:?Задайте MESHKEEPER_DEPLOY_USER}"
TARGET="$USER@$HOST"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BINARY="$ROOT/dist/server/meshkeeper-node"
PUBLIC="$ROOT/dist/public"

# Адрес, который слушает узел. Наружу он не смотрит — его публикует обратный
# прокси, — но порт приходится задавать: на этой машине 8080 занят чужим
# сервисом, поэтому по умолчанию 8090.
BIND="${MESHKEEPER_BIND:-127.0.0.1:8090}"

# Каталог сервиса. Машина общая: и порт, и путь задаются снаружи, потому
# что значения по умолчанию на ней уже заняты чужим хозяйством.
DIR="${MESHKEEPER_DEPLOY_DIR:-/root/meshkeeper}"

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
ssh -n -o BatchMode=yes "$TARGET" "test -d '$DIR'" || {
  echo "Нет доступа по ключу или не создан $DIR. См. deploy/README.md" >&2
  exit 1
}

# Копию базы снимаем до подмены двоичного файла, а не после: если новая
# версия не поднимется, откатываться будет уже нечем.
echo "→ Снимаю копию базы"
ssh -n "$TARGET" "cd '$DIR' && sqlite3 data/meshkeeper.db \".backup '$DIR/backups/pre-deploy-\$(date +%Y%m%d-%H%M%S).db'\"" 

echo "→ Загружаю новую версию во временный каталог"
ssh -n "$TARGET" "rm -rf '$DIR/incoming' && mkdir -p '$DIR/incoming'"
scp -q "$BINARY" "$TARGET:$DIR/incoming/meshkeeper-node"
scp -qr "$PUBLIC" "$TARGET:$DIR/incoming/public"
# Юнит подставляем с нужным адресом, а не шлём как есть.
UNIT="$(mktemp)"
trap 'rm -f "$UNIT"' EXIT
sed "s|^Environment=MESHKEEPER_BIND=.*|Environment=MESHKEEPER_BIND=$BIND|"   "$ROOT/deploy/meshkeeper.service" > "$UNIT"
grep -q "MESHKEEPER_BIND=$BIND" "$UNIT" || {
  echo "Не удалось подставить адрес $BIND в юнит" >&2
  exit 1
}
scp -q "$UNIT" "$TARGET:$DIR/incoming/meshkeeper.service"

echo "→ Переключаю сервис"
# Единственная команда, разрешённая деплой-пользователю через sudo. Она ставится
# скриптом deploy/bootstrap.ps1 и делает переключение целиком, поэтому в sudoers
# не нужны шаблоны с подстановками.
# База не трогается: она живёт в /var/lib/meshkeeper и переживает выкладку.
ssh -n "$TARGET" 'sudo /usr/local/sbin/meshkeeper-activate'

echo "→ Проверяю здоровье"
ssh -n "$TARGET" "curl -fsS http://$BIND/health" && echo
echo "Готово."
