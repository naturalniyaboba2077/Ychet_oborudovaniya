# Забирает резервные копии с сервера на этот компьютер.
#
# Зачем отдельно: копии, лежащие рядом с базой, не переживают смерть машины.
# В августе так и вышло — сервер пропал вместе с базой И со всеми копиями,
# потому что хранились они на том же диске. Копия имеет смысл только там,
# где её не достанет то же событие.
#
# Запускается по расписанию (см. ниже) либо руками:
#   ./deploy/pull-backups.ps1
#
# Ключ доступа и адрес берутся из ~/.ssh/config по имени хоста.

param(
    # Имя записи Host в ~/.ssh/config.
    [string]$Alias = 'meshkeeper',

    # Каталог на сервере с копиями.
    [string]$Remote = '/root/meshkeeper/backups',

    # Куда складывать здесь.
    [string]$Local = "$env:USERPROFILE\Desktop\Секреты MeshKeeper\копии базы",

    # Сколько копий держать на этом компьютере.
    [int]$Keep = 30
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Local)) {
    New-Item -ItemType Directory -Path $Local -Force | Out-Null
}

Write-Host "Забираю копии с $Alias" -ForegroundColor Cyan

# -n обязателен: иначе ssh забирает консольный ввод и задача висит вечно.
$names = ssh -n -o BatchMode=yes -o ConnectTimeout=20 $Alias "ls -1 $Remote 2>/dev/null"
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось получить список копий с $Alias. Проверьте: ssh $Alias"
}
$names = $names | Where-Object { $_ -match '\.enc$|\.gz$' }
if (-not $names) {
    Write-Host 'На сервере копий нет' -ForegroundColor Yellow
    exit 0
}

$new = 0
foreach ($name in $names) {
    $target = Join-Path $Local $name
    if (Test-Path $target) { continue }
    scp -q "${Alias}:$Remote/$name" $target
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "Не скопировалось: $name"
        continue
    }
    # Пустая копия — это ровно та беда, что была в сентябре: файл есть,
    # внутри ничего. Лучше сказать сразу, чем обнаружить при восстановлении.
    $size = (Get-Item $target).Length
    if ($size -lt 1024) {
        Write-Warning "Копия подозрительно мала ($size б): $name"
    }
    $new++
    Write-Host "  + $name ($size б)" -ForegroundColor Green
}

Write-Host "Новых копий: $new"

# Ротация: держим последние $Keep, остальное убираем.
$all = Get-ChildItem $Local -File | Where-Object { $_.Name -match '^meshkeeper-.*\.(enc|gz)$' } |
    Sort-Object LastWriteTime -Descending
if ($all.Count -gt $Keep) {
    $all | Select-Object -Skip $Keep | ForEach-Object {
        Remove-Item $_.FullName -Force
        Write-Host "  - удалена старая: $($_.Name)" -ForegroundColor DarkGray
    }
}

Write-Host "Всего копий здесь: $((Get-ChildItem $Local -File | Where-Object { $_.Name -match '^meshkeeper-' }).Count)"

# ─── Как поставить на расписание ────────────────────────────────────────────
#
# Раз в сутки в 20:00, от текущего пользователя:
#
#   $s = New-ScheduledTaskAction -Execute 'powershell.exe' `
#         -Argument '-NoProfile -WindowStyle Hidden -File "ПУТЬ\deploy\pull-backups.ps1"'
#   $t = New-ScheduledTaskTrigger -Daily -At 20:00
#   Register-ScheduledTask -TaskName 'MeshKeeper: забрать копии' -Action $s -Trigger $t
#
# Задача не требует прав администратора: она только читает по SSH-ключу.
