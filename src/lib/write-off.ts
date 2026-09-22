/**
 * Отсрочка списания.
 *
 * Списание — единственное необратимое действие в каталоге, и ошибаются в нём
 * чаще всего: выделили пачку, нажали, а один предмет попал туда случайно.
 * Поэтому между нажатием и исчезновением из каталога есть 15 минут, в
 * которые предмет виден серым с обратным отсчётом и возвращается одной
 * кнопкой.
 *
 * Число минут здесь должно совпадать с `WRITE_OFF_GRACE_MINUTES`
 * в `backend/src/api/items.rs`: сервер по нему решает, показывать предмет
 * в каталоге или уже в архиве. Разойдутся — человек увидит таймер на
 * предмете, которого в списке уже нет.
 */
import { useEffect, useReducer } from 'react'

export const WRITE_OFF_GRACE_MINUTES = 15

/**
 * Сколько миллисекунд осталось до окончательного списания.
 *
 * `null` — предмет не списан. `0` — отсрочка вышла (сервер такой предмет
 * в каталоге уже не отдаёт, но список на экране может быть на минуту
 * устаревшим, и рисовать отрицательное время нельзя).
 */
export function writeOffMsLeft(
  writtenOffAt: string | null | undefined,
  now: number = Date.now(),
): number | null {
  if (!writtenOffAt) return null
  const at = Date.parse(normalizeStamp(writtenOffAt))
  if (Number.isNaN(at)) return null
  const left = at + WRITE_OFF_GRACE_MINUTES * 60_000 - now
  return left > 0 ? left : 0
}

/**
 * Сервер пишет отметку в RFC 3339 со смещением — такую `Date.parse`
 * разбирает везде одинаково. Но в базе рядом лежат строки в формате SQLite
 * («2026-09-22 14:03:11», без зоны): они приезжают обменом с другого узла и
 * остались от прежних версий. Chrome читает их как местное время, хотя это
 * UTC, а Safari не читает вовсе — таймер показал бы три часа вместо
 * пятнадцати минут либо не показался бы совсем.
 */
function normalizeStamp(raw: string): string {
  const trimmed = raw.trim()
  if (/\d{2}:\d{2}(:\d{2})?$/.test(trimmed) && !/[zZ]|[+-]\d{2}:?\d{2}$/.test(trimmed)) {
    return trimmed.replace(' ', 'T') + 'Z'
  }
  return trimmed
}

/** «14:59», «0:07» — то, что видно на карточке. */
export function formatMsLeft(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000))
  const minutes = Math.floor(total / 60)
  const seconds = total % 60
  return `${minutes}:${String(seconds).padStart(2, '0')}`
}

/**
 * Живой отсчёт для карточки.
 *
 * Тикает раз в секунду и только пока есть что отсчитывать: таймер на
 * каждой из сотни карточек каталога, крутящийся вхолостую, заметно греет
 * телефон.
 */
export function useWriteOffCountdown(writtenOffAt: string | null | undefined): number | null {
  // Остаток не храним в состоянии, а считаем при отрисовке: хранимую копию
  // пришлось бы сбрасывать при смене отметки, то есть звать setState прямо
  // в эффекте — лишний круг отрисовки на каждой карточке каталога.
  // Состояние тут нужно ровно одно: «пора перерисовать».
  const [, tick] = useReducer((n: number) => n + 1, 0)

  useEffect(() => {
    if (!writtenOffAt) return
    const id = window.setInterval(() => {
      tick()
      // Дальше отсчитывать нечего: ноль уже показан.
      if ((writeOffMsLeft(writtenOffAt) ?? 0) <= 0) window.clearInterval(id)
    }, 1000)
    return () => window.clearInterval(id)
  }, [writtenOffAt])

  return writeOffMsLeft(writtenOffAt)
}
