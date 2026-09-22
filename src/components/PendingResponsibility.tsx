/**
 * «За вами хотят закрепить» — список предметов, ждущих согласия человека.
 *
 * Ответственным можно назначить кого угодно, но предмет не выдаётся, пока
 * назначенный не согласится. Иначе выходило бы, что человек узнаёт о своей
 * ответственности, когда с него спросят за пропажу.
 *
 * Уведомление о назначении приходит, но одного уведомления мало: ленту
 * листают, и предмет застревал бы неподтверждённым. Поэтому список висит
 * на «Моих инструментах», пока в нём есть хоть что-то.
 */
import { useState } from 'react'
import { Link } from 'react-router'
import { Check, Loader2, UserCheck, X } from 'lucide-react'
import { AnimatePresence, motion } from 'framer-motion'
import { trpc } from '@/providers/trpc'
import { mapItemToCatalogTool } from '@/lib/catalog-item'

export default function PendingResponsibility({
  onDone,
}: {
  /** Сообщить наружу — обновить счётчики и показать подсказку. */
  onDone?: (message: string) => void
}) {
  const utils = trpc.useUtils()
  const q = trpc.items.list.useQuery({ pendingMine: true, page: 1, limit: 50 }, { retry: 0 })
  const [busyId, setBusyId] = useState<number | null>(null)
  const confirm = trpc.items.confirmResponsibility.useMutation()

  const items = (q.data?.rows ?? []).map(mapItemToCatalogTool)
  if (items.length === 0) return null

  const decide = async (id: number, name: string, accept: boolean) => {
    setBusyId(id)
    try {
      await confirm.mutateAsync({ id, accept })
      await utils.items.list.invalidate()
      await utils.notifications.unreadCount.invalidate()
      onDone?.(accept ? `«${name}» закреплён за вами` : `Вы отказались от «${name}»`)
    } catch (e) {
      onDone?.(e instanceof Error ? e.message : 'Не получилось')
    } finally {
      setBusyId(null)
    }
  }

  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.24 }}
      className="rounded-card border border-amber-300/70 bg-amber-50 p-4"
    >
      <h2 className="flex items-center gap-2 text-[15px] font-semibold text-amber-900">
        <UserCheck size={17} strokeWidth={2} />
        Ждут вашего подтверждения
        <span className="font-mono-num">({items.length})</span>
      </h2>
      <p className="mt-1 text-[13px] leading-5 text-amber-800">
        Пока вы не согласитесь, предмет за вами не числится и никому не выдаётся.
      </p>

      <ul className="mt-3 space-y-2">
        <AnimatePresence initial={false}>
          {items.map((tool) => (
            <motion.li
              key={tool.id}
              layout
              exit={{ opacity: 0, height: 0 }}
              className="flex flex-wrap items-center gap-2 rounded-xl border border-amber-200 bg-white px-3 py-2"
            >
              <Link
                to={`/tool/${tool.numericId}`}
                className="min-w-0 flex-1 text-sm font-semibold text-ink-900 hover:text-brand-600"
              >
                <span className="font-mono-num text-ink-500">{tool.vn}</span> {tool.name}
              </Link>
              <button
                onClick={() => decide(tool.numericId, tool.name, true)}
                disabled={busyId === tool.numericId}
                className="inline-flex h-8 items-center gap-1.5 rounded-xl bg-accent px-3.5 text-[13px] font-semibold text-white transition hover:bg-accent-hover disabled:opacity-60"
              >
                {busyId === tool.numericId ? (
                  <Loader2 size={14} className="animate-spin" />
                ) : (
                  <Check size={14} />
                )}
                Беру
              </button>
              <button
                onClick={() => decide(tool.numericId, tool.name, false)}
                disabled={busyId === tool.numericId}
                className="inline-flex h-8 items-center gap-1.5 rounded-xl border border-brand-100 bg-white px-3.5 text-[13px] font-semibold text-ink-900 transition hover:bg-brand-50 disabled:opacity-60"
              >
                <X size={14} />
                Отказаться
              </button>
            </motion.li>
          ))}
        </AnimatePresence>
      </ul>
    </motion.section>
  )
}
