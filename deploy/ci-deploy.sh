#!/usr/bin/env bash
# Приём выкладки из GitHub Actions (.github/workflows/release.yml).
#
# Ставится как /usr/local/sbin/meshkeeper-ci-deploy и вызывается только как
# принудительная команда SSH-ключа выкладки:
#   command="/usr/local/sbin/meshkeeper-ci-deploy",restrict ssh-ed25519 … meshkeeper-ci
# Ключ ничего другого на сервере сделать не может: ни shell, ни проброса.
#
# На stdin приходит tar.gz: meshkeeper-node, public/ и, если собирался,
# android/meshkeeper-<N>.apk. Перед переключением снимается копия базы; если
# новая версия не поднялась, возвращаем прежнюю.
set -euo pipefail
umask 022

ROOT=/root/meshkeeper
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
WORK=$(mktemp -d "$ROOT/.ci-XXXXXX")
trap 'rm -rf "$WORK"' EXIT

cd "$ROOT"
[ -e incoming ] && { echo "incoming/ уже занят — идёт другая выкладка?" >&2; exit 1; }

# Пакет не больше 200 МБ: защита от переполнения диска обрывком или ошибкой.
head -c 209715200 | tar -xz -C "$WORK" --no-same-owner --no-same-permissions

[ -f "$WORK/meshkeeper-node" ] || { echo "в пакете нет meshkeeper-node" >&2; exit 1; }
[ -f "$WORK/public/index.html" ] || { echo "в пакете нет public/index.html" >&2; exit 1; }
head -c 4 "$WORK/meshkeeper-node" | grep -q $'\x7fELF' || { echo "meshkeeper-node не ELF" >&2; exit 1; }

APK=""
if [ -d "$WORK/android" ]; then
    APK=$(find "$WORK/android" -maxdepth 1 -name 'meshkeeper-*.apk' | head -n1)
    [ -n "$APK" ] || { echo "android/ без APK" >&2; exit 1; }
    code=$(basename "$APK" .apk); code=${code#meshkeeper-}
    [[ "$code" =~ ^[0-9]+$ ]] || { echo "плохое имя APK: $APK" >&2; exit 1; }
    last=$(ls apk 2>/dev/null | sed -n 's/^meshkeeper-\([0-9]\+\)\.apk$/\1/p' | sort -n | tail -n1)
    if [ -n "$last" ] && [ "$code" -le "$last" ]; then
        echo "APK $code не новее выложенного $last" >&2; exit 1
    fi
fi

# Копия базы: через .backup, а не cp — в режиме WAL данные ещё в -wal.
sqlite3 data/meshkeeper.db ".backup 'backups/pre-deploy-$STAMP.db'"
echo "база сохранена: backups/pre-deploy-$STAMP.db"

# Прежняя версия — на случай, если новая не поднимется.
rm -rf .rollback && mkdir .rollback
cp -a meshkeeper-node .rollback/ && cp -a public .rollback/

mkdir incoming
install -m 0755 "$WORK/meshkeeper-node" incoming/meshkeeper-node
mv "$WORK/public" incoming/public

healthy() {
    for _ in $(seq 1 15); do
        curl -fsS -m 3 http://127.0.0.1:8090/health >/dev/null 2>&1 && return 0
        sleep 1
    done
    return 1
}

if ! /usr/local/sbin/meshkeeper-activate || ! healthy; then
    echo "новая версия не поднялась, возвращаю прежнюю" >&2
    rm -rf incoming && mkdir incoming
    cp -a .rollback/meshkeeper-node .rollback/public incoming/
    /usr/local/sbin/meshkeeper-activate >&2 || true
    exit 1
fi

# APK выкладываем после сервера: телефоны начнут качать его сразу.
# .part → mv, чтобы apk.rs не отдал недокопированный файл.
if [ -n "$APK" ]; then
    name=$(basename "$APK")
    cp "$APK" "apk/$name.part" && mv "apk/$name.part" "apk/$name"
    echo "APK выложен: apk/$name"
fi

echo DEPLOY_OK
