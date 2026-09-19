import { cn } from '@/lib/utils'

/**
 * Статус предмета в том виде, в каком его отдаёт сервер.
 *
 * Названия и цвета статусов настраиваются в каждой группе своими, поэтому
 * рисовать их по зашитому в клиент списку нельзя: свой статус показался бы
 * чужим именем и чужим цветом. Раньше здесь так и было — список брался из
 * mock-data, — но страницы этим не пользовались и рисовали статус сами,
 * каждая по-своему. Общий вид живёт здесь.
 */
export type ApiStatus = { name: string; color: string; bg: string } | null | undefined

/** Цветная точка + подпись статуса. */
export function StatusDot({ status, className }: { status: ApiStatus; className?: string }) {
  if (!status) return <span className={cn('text-xs text-ink-300', className)}>—</span>
  return (
    <span
      className={cn('inline-flex items-center gap-1.5 text-xs font-semibold', className)}
      style={{ color: status.color }}
    >
      <span className="w-1.5 h-1.5 rounded-full shrink-0" style={{ background: status.color }} />
      {status.name}
    </span>
  )
}

/** Статусный бейдж-пилюля: цветная пара фон/текст из настроек группы. */
export function StatusBadge({
  status,
  className,
  emptyClassName = 'text-xs text-ink-300',
}: {
  status: ApiStatus
  className?: string
  /** Размер прочерка отличается в карточке и в списке. */
  emptyClassName?: string
}) {
  if (!status) return <span className={cn(emptyClassName, className)}>—</span>
  return (
    <span
      className={cn('inline-flex items-center rounded-full px-2.5 py-0.5 text-caption', className)}
      style={{ background: status.bg, color: status.color }}
    >
      {status.name}
    </span>
  )
}

/** QR-бейдж (teal) */
export function QrBadge({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-full bg-teal/20 px-2 py-0.5 text-[11px] font-semibold text-teal-dark',
        className
      )}
    >
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
        <rect x="3" y="3" width="7" height="7" rx="1" />
        <rect x="14" y="3" width="7" height="7" rx="1" />
        <rect x="3" y="14" width="7" height="7" rx="1" />
        <path d="M14 14h3v3h-3zM21 14v.01M14 21v.01M21 21v.01M18 18v.01" />
      </svg>
      QR
    </span>
  )
}

/** Бейдж «Материалы» (количественный учёт) */
export function MaterialBadge({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-full bg-brand-100/60 px-2 py-0.5 text-[11px] font-semibold text-ink-500',
        className
      )}
    >
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
        <path d="M21 8l-9-5-9 5v8l9 5 9-5V8z" />
        <path d="M3 8l9 5 9-5M12 13v8" />
      </svg>
      Материалы
    </span>
  )
}
