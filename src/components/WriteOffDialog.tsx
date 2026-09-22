/**
 * Подтверждение списания.
 *
 * Списание — самое тяжёлое действие в каталоге: предмет гаснет, а через
 * четверть часа уходит в архив. Поэтому спрашиваем причину (иначе через
 * месяц никто не вспомнит, куда делся перфоратор) и прямо говорим, что
 * вернуть можно и потом.
 *
 * Общий на каталог и «Мои инструменты»: в «Моих» кнопка списания раньше
 * просто показывала надпись «Списание доступно руководителю» — и она врала,
 * потому что руководителю оно тоже не срабатывало.
 */
import { AnimatePresence, motion } from 'framer-motion'
import { WRITE_OFF_GRACE_MINUTES } from '@/lib/write-off'

export default function WriteOffDialog({
  open,
  count,
  reason,
  onReasonChange,
  busy,
  onCancel,
  onConfirm,
}: {
  open: boolean
  count: number
  reason: string
  onReasonChange: (v: string) => void
  busy: boolean
  onCancel: () => void
  onConfirm: () => void
}) {
  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-[70] flex items-center justify-center bg-ink-900/40 px-4"
          onClick={() => !busy && onCancel()}
        >
          <motion.div
            initial={{ opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.96, y: 12 }}
            onClick={(e) => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            className="w-full max-w-md rounded-card bg-surface p-5 shadow-modal"
          >
            <h3 className="text-lg font-bold text-ink-900">
              Списать {count} {count === 1 ? 'предмет' : 'предметов'}?
            </h3>
            <p className="mt-2 text-sm leading-5 text-ink-500">
              Предметы погаснут в каталоге и через {WRITE_OFF_GRACE_MINUTES} минут уйдут в архив.
              История выдач сохранится, вернуть их можно и потом.
            </p>
            <label className="mt-4 block text-[13px] font-semibold text-ink-900">
              Причина списания
            </label>
            <input
              autoFocus
              value={reason}
              onChange={(e) => onReasonChange(e.target.value)}
              placeholder="Сломан, утерян, изношен…"
              className="mt-1.5 h-11 w-full rounded-xl border border-brand-100 bg-surface px-3 text-sm text-ink-900 outline-none focus:border-brand-600 focus:ring-[3px] focus:ring-brand-600/15"
            />
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                disabled={busy}
                onClick={onCancel}
                className="h-10 rounded-xl border border-brand-100 px-4 text-sm font-semibold text-ink-900 hover:bg-brand-50 disabled:opacity-60"
              >
                Отмена
              </button>
              <button
                type="button"
                disabled={busy || reason.trim().length < 3}
                onClick={onConfirm}
                className="h-10 rounded-xl bg-danger px-4 text-sm font-semibold text-white hover:opacity-90 disabled:opacity-60"
              >
                {busy ? 'Списываем…' : 'Списать'}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
