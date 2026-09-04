/**
 * Сверяет два описания API: настоящее — в Rust, и типовое — в `api/`.
 *
 * Роутеры в `api/` ничего не исполняют, но именно из них выводятся типы
 * клиента. Их писали руками по образу `backend/src/api/`, и разойтись они
 * могут молча: TypeScript проверит, что вызов совпадает с заглушкой, а
 * совпадает ли заглушка с сервером — не проверит никто. На этом уже дважды
 * ловились баги, поэтому расхождение должно ронять сборку, а не всплывать
 * в проде.
 *
 * Список процедур на стороне Rust берётся из ветвей `match` в
 * `dispatch_inner`, на стороне TypeScript — из самого собранного роутера,
 * а не разбором текста: tRPC сам знает свои процедуры.
 */
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { appRouter } from '../api/router'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

/** Процедуры, объявленные в типовом роутере: "auth.login", "items.list"… */
function typescriptProcedures(): Set<string> {
  const def = (appRouter as unknown as { _def: { procedures: Record<string, unknown> } })._def
  return new Set(Object.keys(def.procedures))
}

/**
 * Процедуры, которые действительно обрабатывает Rust.
 *
 * Берём только тело `dispatch_inner`: выше по файлу те же строки встречаются
 * в списках прав и в перечне публичных процедур, и они не означают, что
 * обработчик существует.
 */
function rustProcedures(): Set<string> {
  const source = readFileSync(path.join(root, 'backend', 'src', 'api', 'mod.rs'), 'utf8')
  const start = source.indexOf('fn dispatch_inner')
  if (start < 0) throw new Error('в api/mod.rs не найдена dispatch_inner')
  // Конец — следующая функция верхнего уровня после dispatch_inner.
  const rest = source.slice(start + 1)
  const nextFn = rest.search(/\n(?:pub )?fn /)
  const body = nextFn < 0 ? rest : rest.slice(0, nextFn)

  const found = new Set<string>()
  // Ветви вида: "auth.login" => …  и  "a.b" | "a.c" => …
  for (const line of body.split('\n')) {
    if (!line.includes('=>') && !/^\s*\| "/.test(line)) continue
    for (const m of line.matchAll(/"([a-z][A-Za-z0-9]*(?:\.[A-Za-z0-9]+)+)"/g)) {
      found.add(m[1])
    }
  }
  return found
}

const ts = typescriptProcedures()
const rs = rustProcedures()

// «ping» есть в обоих, но в Rust это ветвь без точки — сверяем только
// составные имена, одиночные к расхождению схем не относятся.
const onlyTs = [...ts].filter((p) => p.includes('.') && !rs.has(p)).sort()
const onlyRs = [...rs].filter((p) => !ts.has(p)).sort()

if (onlyTs.length === 0 && onlyRs.length === 0) {
  console.log(`Схемы API совпадают: ${ts.size} процедур в типах, ${rs.size} в Rust.`)
  process.exit(0)
}

if (onlyTs.length > 0) {
  console.error('Клиент типизирован под процедуры, которых нет в Rust —')
  console.error('вызов пройдёт проверку типов и упадёт в бою:')
  for (const p of onlyTs) console.error(`  ${p}`)
}
if (onlyRs.length > 0) {
  if (onlyTs.length > 0) console.error('')
  console.error('Rust умеет то, о чём не знают типы —')
  console.error('из клиента это недоступно, допишите заглушку в api/:')
  for (const p of onlyRs) console.error(`  ${p}`)
}
process.exit(1)
