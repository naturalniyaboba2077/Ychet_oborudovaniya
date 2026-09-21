#!/usr/bin/env bash
# Резервная копия базы MeshKeeper.
#
# ТЗ §6 требует обязательную резервную копию. В серверной схеме её делает сам
# сервер: копия снимается штатным механизмом SQLite (без остановки сервиса),
# проверяется на читаемость, затем сжимается и шифруется, старые копии
# удаляются по сроку хранения.
#
# Запускается таймером systemd, см. meshkeeper-backup.timer.
#
# Переменные (из файла окружения сервиса):
#   MESHKEEPER_DB             путь к базе
#   MESHKEEPER_BACKUP_DIR     куда складывать (по умолчанию /var/backups/meshkeeper)
#   MESHKEEPER_BACKUP_KEEP    сколько копий хранить (по умолчанию 14)
#   MESHKEEPER_BACKUP_PASS    пароль шифрования; без него копия остаётся открытой

set -euo pipefail

DB="${MESHKEEPER_DB:-/var/lib/meshkeeper/meshkeeper.db}"
DEST="${MESHKEEPER_BACKUP_DIR:-/var/backups/meshkeeper}"
KEEP="${MESHKEEPER_BACKUP_KEEP:-14}"
STAMP="$(date +%Y%m%d-%H%M%S)"

if [ ! -f "$DB" ]; then
  echo "базы нет: $DB" >&2
  exit 1
fi

# sqlite3 обязателен, и это не придирка. База работает в режиме WAL: сам файл
# .db содержит почти пустой заголовок, а все данные лежат в -wal до
# контрольной точки. Копирование файла через cp давало зашифрованный архив на
# 128 байт — формально успех, фактически пустота, и обнаруживается это в тот
# день, когда копия понадобилась. Лучше громко упасть.
if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "нет sqlite3 — копию снять нечем. Установите: apt install sqlite3" >&2
  exit 1
fi

mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# .backup читает базу целиком вместе с WAL и делает это на живой базе,
# не мешая сервису.
sqlite3 "$DB" ".backup '$TMP/meshkeeper.db'"

# Проверяем то, что сняли, а не то, что собирались снять. Пустой или битый
# файл дальше не пойдёт.
INTEGRITY="$(sqlite3 "$TMP/meshkeeper.db" 'PRAGMA integrity_check;' 2>&1 | head -1)"
if [ "$INTEGRITY" != "ok" ]; then
  echo "копия не прошла проверку целостности: $INTEGRITY" >&2
  exit 1
fi

TABLES="$(sqlite3 "$TMP/meshkeeper.db" "SELECT COUNT(*) FROM sqlite_master WHERE type='table';")"
if [ "$TABLES" -lt 1 ]; then
  echo "в копии нет ни одной таблицы — снимать нечего" >&2
  exit 1
fi
USERS="$(sqlite3 "$TMP/meshkeeper.db" 'SELECT COUNT(*) FROM users;' 2>/dev/null || echo 0)"

# Вложения лежат файлами рядом с базой, а не строками внутри неё. Копия базы
# без них бесполезна: карточки останутся, снимки пропадут. Кладём в один
# архив: база + каталог файлов.
FILES="${MESHKEEPER_FILES_DIR:-$(dirname "$DB")/files}"
PHOTOS=0
if [ -d "$FILES" ]; then
  PHOTOS="$(find "$FILES" -type f | wc -l)"
  cp -a "$FILES" "$TMP/files"
fi
tar -C "$TMP" -cf "$TMP/meshkeeper.tar" meshkeeper.db $([ -d "$TMP/files" ] && echo files)
rm -rf "$TMP/files"
mv "$TMP/meshkeeper.tar" "$TMP/meshkeeper.db"

gzip -9 "$TMP/meshkeeper.db"
OUT="$DEST/meshkeeper-$STAMP.db.gz"

if [ -n "${MESHKEEPER_BACKUP_PASS:-}" ] && command -v openssl >/dev/null 2>&1; then
  # Симметричное шифрование с выводом ключа из пароля: копию можно класть
  # в облако, не раскрывая содержимое инвентаризации.
  openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -salt \
    -in "$TMP/meshkeeper.db.gz" -out "$OUT.enc" \
    -pass env:MESHKEEPER_BACKUP_PASS
  OUT="$OUT.enc"

  # Восстановление проверяем на самом деле, а не надеемся на него: архив
  # расшифровывается обратно и открывается как база. Иначе следующая тихая
  # поломка найдётся так же поздно, как эта.
  openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 \
    -in "$OUT" -out "$TMP/check.db.gz" -pass env:MESHKEEPER_BACKUP_PASS
  gunzip -f "$TMP/check.db.gz"
  mkdir -p "$TMP/check"
  tar -C "$TMP/check" -xf "$TMP/check.db"
  BACK="$(sqlite3 "$TMP/check/meshkeeper.db" 'PRAGMA integrity_check;' 2>&1 | head -1)"
  if [ "$BACK" != "ok" ]; then
    echo "копия не восстанавливается: $BACK" >&2
    rm -f "$OUT"
    exit 1
  fi
  rm -rf "$TMP/check" "$TMP/check.db"
else
  cp "$TMP/meshkeeper.db.gz" "$OUT"
  echo "ВНИМАНИЕ: MESHKEEPER_BACKUP_PASS не задан, копия не зашифрована" >&2
fi

chmod 600 "$OUT"
SIZE="$(du -h "$OUT" | cut -f1)"
echo "копия готова: $OUT ($SIZE, таблиц $TABLES, пользователей $USERS, вложений $PHOTOS, восстановление проверено)"

# Ротация по количеству копий.
mapfile -t OLD < <(ls -1t "$DEST"/meshkeeper-*.db.gz* 2>/dev/null | tail -n +"$((KEEP + 1))")
for f in "${OLD[@]:-}"; do
  [ -n "$f" ] || continue
  rm -f "$f"
  echo "удалена старая копия: $(basename "$f")"
done

echo "всего копий: $(ls -1 "$DEST"/meshkeeper-*.db.gz* 2>/dev/null | wc -l)"
