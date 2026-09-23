// Организация, выбранная в интерфейсе.
//
// Её id уходит с каждым запросом в заголовке x-mk-workspace (см. trpc.tsx):
// многие процедуры не принимают workspaceId, и без заголовка сервер не знал
// бы, в какой из организаций человек сейчас работает, — права и данные шли
// бы из другой. Сервер проверяет членство, так что заголовок — пожелание,
// а не пропуск.
//
// Выбор запоминается на устройстве: после перезапуска приложение открывается
// в той же организации, а не в первой по списку.

const KEY = 'mk-workspace'

function read(): number | null {
  try {
    const id = Number(localStorage.getItem(KEY))
    return Number.isInteger(id) && id > 0 ? id : null
  } catch {
    return null
  }
}

let current: number | null = read()

export function activeWorkspaceId(): number | null {
  return current
}

export function rememberActiveWorkspace(id: number | null): void {
  current = id
  try {
    if (id) localStorage.setItem(KEY, String(id))
    else localStorage.removeItem(KEY)
  } catch {
    /* приватный режим: помним до перезагрузки */
  }
}
