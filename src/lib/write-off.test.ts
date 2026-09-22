/**
 * Отсчёт отсрочки списания.
 *
 * Проверять это руками нельзя: чтобы увидеть конец отсрочки, надо ждать
 * пятнадцать минут, и разбор времени с зонами тут ошибается молча — таймер
 * просто показывает не то число, а понять это по экрану невозможно.
 */
import { describe, it, expect } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { writeOffMsLeft, formatMsLeft, WRITE_OFF_GRACE_MINUTES } from '@/lib/write-off'

const MIN = 60_000

describe('отсрочка одна и та же на клиенте и на сервере', () => {
  it('совпадает с WRITE_OFF_GRACE_MINUTES в Rust', () => {
    // Разойдутся — человек увидит таймер на предмете, которого в списке
    // уже нет, либо предмет исчезнет раньше, чем таймер дотикает. Обе
    // половины правки легко сделать по отдельности и забыть про вторую.
    const source = readFileSync(
      path.resolve(process.cwd(), 'backend/src/api/items.rs'),
      'utf8',
    )
    const found = source.match(/WRITE_OFF_GRACE_MINUTES:\s*i64\s*=\s*(\d+)/)
    expect(found, 'в items.rs не найдена константа отсрочки').not.toBeNull()
    expect(Number(found![1])).toBe(WRITE_OFF_GRACE_MINUTES)
  })
})

describe('writeOffMsLeft', () => {
  it('не списан — отсчёта нет', () => {
    expect(writeOffMsLeft(null)).toBeNull()
    expect(writeOffMsLeft(undefined)).toBeNull()
    expect(writeOffMsLeft('')).toBeNull()
  })

  it('сразу после списания остаётся вся отсрочка', () => {
    const now = Date.parse('2026-09-22T14:00:00Z')
    expect(writeOffMsLeft('2026-09-22T14:00:00+00:00', now)).toBe(WRITE_OFF_GRACE_MINUTES * MIN)
  })

  it('считает остаток в середине отсрочки', () => {
    const now = Date.parse('2026-09-22T14:10:00Z')
    expect(writeOffMsLeft('2026-09-22T14:00:00+00:00', now)).toBe(5 * MIN)
  })

  it('после отсрочки отдаёт ноль, а не отрицательное время', () => {
    // Список на экране может быть на минуту старше сервера. Отрицательный
    // остаток нарисовался бы как «-1:03».
    const now = Date.parse('2026-09-22T14:30:00Z')
    expect(writeOffMsLeft('2026-09-22T14:00:00+00:00', now)).toBe(0)
  })

  it('строку без часового пояса считает временем UTC', () => {
    // Формат SQLite приезжает обменом с другого узла. Если счесть его
    // местным временем, в Москве отсрочка «закончится» три часа назад.
    const now = Date.parse('2026-09-22T14:05:00Z')
    expect(writeOffMsLeft('2026-09-22 14:00:00', now)).toBe(10 * MIN)
  })

  it('на мусорной дате не падает и таймер не рисует', () => {
    expect(writeOffMsLeft('позавчера')).toBeNull()
  })
})

describe('formatMsLeft', () => {
  it('секунды дополняет нулём', () => {
    expect(formatMsLeft(7_000)).toBe('0:07')
    expect(formatMsLeft(63_000)).toBe('1:03')
  })

  it('полную отсрочку показывает как 15:00', () => {
    expect(formatMsLeft(WRITE_OFF_GRACE_MINUTES * MIN)).toBe('15:00')
  })

  it('ноль и отрицательное — это 0:00', () => {
    expect(formatMsLeft(0)).toBe('0:00')
    expect(formatMsLeft(-5_000)).toBe('0:00')
  })
})
