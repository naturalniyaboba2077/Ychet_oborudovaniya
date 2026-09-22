import { useRef } from 'react'
import { useNavigate } from 'react-router'
import { Building2, Clock, Phone, RotateCcw, UserCheck, Warehouse } from 'lucide-react'
import { motion } from 'framer-motion'
import { cn } from '@/lib/utils'
import type { CatalogTool } from '@/lib/catalog-item'
import { QrBadge, MaterialBadge } from '@/components/StatusBadge'
import { useStore } from '@/lib/store'
import { formatMsLeft, useWriteOffCountdown } from '@/lib/write-off'

interface Props {
  tool: CatalogTool
  selectionMode?: boolean
  onCallClick?: (tool: CatalogTool) => void
  /** Показывается на списанном предмете, пока идёт отсрочка. */
  onRestore?: (tool: CatalogTool) => void
  restoring?: boolean
}

export default function ToolMiniCard({
  tool,
  selectionMode = false,
  onCallClick,
  onRestore,
  restoring = false,
}: Props) {
  const navigate = useNavigate()
  const { selectedToolIds, toggleToolSelected, setSelectionMode } = useStore()
  const selected = selectedToolIds.has(tool.id)

  // Списанный предмет ещё виден, но уже не живой: серый, с обратным
  // отсчётом и кнопкой вернуть. Выделять его в пачку незачем — списывать
  // второй раз нечего.
  const msLeft = useWriteOffCountdown(tool.writtenOffAt)
  const writtenOff = msLeft !== null

  const openCard = () => navigate(`/tool/${tool.numericId}`)

  const onCheck = (e: React.MouseEvent) => {
    e.stopPropagation()
    if (!selectionMode && !selected) setSelectionMode(true)
    toggleToolSelected(tool.id)
  }

  // Долгое нажатие включает выделение — привычный на телефоне жест, где
  // маленькую галочку попасть трудно. Обычное нажатие по-прежнему открывает
  // карточку: чтобы одно не срабатывало вместо другого, после долгого
  // нажатия ближайший клик подавляется.
  const holdTimer = useRef<number | null>(null)
  const heldRef = useRef(false)

  const startHold = () => {
    if (writtenOff) return
    heldRef.current = false
    holdTimer.current = window.setTimeout(() => {
      heldRef.current = true
      if (!selectionMode) setSelectionMode(true)
      toggleToolSelected(tool.id)
      // Короткая отдача: человек должен понять, что жест принят.
      try {
        navigator.vibrate?.(30)
      } catch {
        /* вибрация есть не везде */
      }
    }, 450)
  }

  const cancelHold = () => {
    if (holdTimer.current !== null) {
      window.clearTimeout(holdTimer.current)
      holdTimer.current = null
    }
  }

  const onCardClick = () => {
    if (heldRef.current) {
      heldRef.current = false
      return
    }
    if (selectionMode && !writtenOff) {
      toggleToolSelected(tool.id)
      return
    }
    openCard()
  }

  return (
    <motion.article
      onClick={onCardClick}
      onPointerDown={startHold}
      onPointerUp={cancelHold}
      onPointerLeave={cancelHold}
      onPointerCancel={cancelHold}
      onContextMenu={(e) => e.preventDefault()}
      whileHover={{ y: -2 }}
      transition={{ duration: 0.2, ease: 'easeOut' }}
      className={cn(
        'group relative bg-surface rounded-mini border shadow-card p-3 cursor-pointer transition-shadow hover:shadow-hover',
        selected ? 'border-brand-600 ring-2 ring-brand-600/20' : 'border-brand-100/60',
        writtenOff && 'border-dashed border-ink-300/70 bg-brand-50/40 shadow-none',
      )}
    >
      <div
        className={cn(
          'relative overflow-hidden rounded-[10px] aspect-[4/3] bg-brand-50',
          writtenOff && 'grayscale opacity-55',
        )}
      >
        <img
          src={tool.photo}
          alt={tool.name}
          loading="lazy"
          className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-[1.03]"
        />
        <button
          onClick={onCheck}
          aria-label={selected ? 'Снять выбор' : 'Выбрать'}
          className={cn(
            'absolute left-2 top-2 w-5 h-5 rounded-md border-[1.5px] flex items-center justify-center transition-all duration-150',
            'bg-white/80 backdrop-blur-sm',
            selected
              ? 'bg-brand-600 border-brand-600 opacity-100'
              : 'border-brand-100 opacity-0 group-hover:opacity-100',
            (selectionMode || selected) && 'opacity-100',
            writtenOff && 'hidden',
          )}
        >
          {selected && (
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
              <path d="M2 6.2L4.8 9L10 3" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          )}
        </button>
        {(tool.hasQr || tool.isMaterial) && (
          <div className="absolute right-2 bottom-2 flex gap-1">
            {tool.hasQr && <QrBadge />}
            {tool.isMaterial && <MaterialBadge />}
          </div>
        )}
      </div>

      {/* Полоса списания. Живёт над остальным содержимым, чтобы её нельзя
          было не заметить: у человека пятнадцать минут на то, чтобы
          передумать, и в каталоге на сотню карточек бледная подпись внизу
          теряется. */}
      {writtenOff && (
        <div className="mt-2.5 rounded-xl border border-danger/40 bg-danger-bg px-2.5 py-2">
          {/* Подпись короткая не ради красоты: на телефоне карточки идут в
              два столбца, и полоса шириной 144 точки. Полная фраза «в архив
              через 14:15» в неё не влезает — она либо переносится так, что
              время отрывается от «через» и повисает само, либо, если
              запретить перенос, просто обрезается справа. */}
          <div className="flex items-start gap-1.5 text-[12px] font-semibold leading-4 text-danger">
            <Clock size={13} strokeWidth={2} className="mt-0.5 shrink-0" />
            {msLeft > 0 ? (
              <span className="min-w-0">
                Списан
                <span className="block whitespace-nowrap">
                  в архив:{' '}
                  <span className="font-mono-num tabular-nums">{formatMsLeft(msLeft)}</span>
                </span>
              </span>
            ) : (
              // Ноль означает и «отсрочка только что вышла», и «открыт
              // архив». Формулировка должна быть верна в обоих случаях.
              <span>Списан · в архиве</span>
            )}
          </div>
          {onRestore && (
            <button
              onClick={(e) => {
                e.stopPropagation()
                onRestore(tool)
              }}
              disabled={restoring}
              className="mt-1.5 inline-flex items-center gap-1.5 h-7 px-2.5 rounded-lg bg-white border border-danger/40 text-[12px] font-semibold text-danger hover:bg-white/70 disabled:opacity-60 transition-colors"
            >
              <RotateCcw size={12} strokeWidth={2} />
              {restoring ? 'Возвращаем…' : 'Вернуть'}
            </button>
          )}
        </div>
      )}

      {/* Ответственного назначили, но он ещё не согласился. До согласия
          предмет не выдаётся, и знать об этом надо до того, как за ним
          пришли. */}
      {!writtenOff && tool.pendingResponsibleId != null && (
        <div className="mt-2.5 flex items-center gap-1.5 rounded-xl border border-amber-300/70 bg-amber-50 px-2.5 py-1.5 text-[12px] font-semibold text-amber-800">
          <UserCheck size={13} strokeWidth={2} className="shrink-0" />
          Ждёт подтверждения ответственного
        </div>
      )}

      <div className={cn('pt-2.5 space-y-1.5', writtenOff && 'opacity-60')}>
        <div className="flex items-center justify-between gap-2">
          <span className="font-mono-num text-ink-500">{tool.vn}</span>
          <span className="inline-flex items-center gap-1.5 text-xs font-semibold" style={{ color: tool.statusColor }}>
            <span className="w-1.5 h-1.5 rounded-full shrink-0" style={{ background: tool.statusColor }} />
            {tool.statusName}
          </span>
        </div>
        <h3 className="text-[15px] leading-[22px] font-semibold text-ink-900 line-clamp-2 min-h-[44px]">{tool.name}</h3>

        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-ink-500">
          {tool.siteName && (
            <span className="inline-flex items-center gap-1 min-w-0">
              <Building2 size={13} strokeWidth={1.75} className="shrink-0 text-ink-300" />
              <span className="truncate">{tool.siteName}</span>
            </span>
          )}
          {tool.warehouseName && (
            <span className="inline-flex items-center gap-1 min-w-0">
              <Warehouse size={13} strokeWidth={1.75} className="shrink-0 text-ink-300" />
              <span className="truncate">{tool.warehouseName}</span>
            </span>
          )}
          {(tool.totalQty ?? 0) > 0 && (
            <span className="font-mono-num text-ink-500">
              на складе {tool.stockQty ?? 0}
              {(tool.issuedQty ?? 0) > 0 ? ` · выдано ${tool.issuedQty}` : ''}
              {tool.isMaterial && tool.unit ? ` ${tool.unit}` : ' шт'}
            </span>
          )}
        </div>

        <div className="flex items-center gap-2 pt-1 border-t border-brand-100/60">
          {tool.holders && tool.holders.length > 0 ? (
            <div className="min-w-0 flex-1">
              <div className="text-[11px] text-ink-500">Сейчас у</div>
              <div className="text-xs font-semibold text-ink-900 truncate">
                {tool.holders
                  .map((h) => {
                    const name = h.user?.fullName ?? 'сотрудник'
                    const q = h.quantity && h.quantity !== 1 ? ` ×${h.quantity}` : ''
                    const vn = h.internalId ? ` (${h.internalId})` : ''
                    return name + q + vn
                  })
                  .join(', ')}
              </div>
            </div>
          ) : tool.assigneeName ? (
            <>
              {tool.assigneeAvatar ? (
                <img
                  src={tool.assigneeAvatar}
                  alt={tool.assigneeName}
                  className="w-5 h-5 rounded-full object-cover border border-brand-100"
                />
              ) : (
                <span className="w-5 h-5 rounded-full bg-brand-100" />
              )}
              <span className="text-xs font-semibold text-ink-900 truncate flex-1">{tool.assigneeName}</span>
              {tool.assigneePhone && (
                <button
                  onClick={(e) => {
                    e.stopPropagation()
                    onCallClick?.(tool)
                    window.location.href = `tel:${tool.assigneePhone!.replace(/[^+\d]/g, '')}`
                  }}
                  aria-label={`Позвонить: ${tool.assigneeName}`}
                  className="w-8 h-8 shrink-0 flex items-center justify-center rounded-lg text-teal-dark hover:bg-teal/15 transition-colors"
                >
                  <Phone size={15} strokeWidth={1.75} />
                </button>
              )}
            </>
          ) : (
            <span className="text-xs text-ink-300 py-1.5">На складе</span>
          )}
        </div>
      </div>
    </motion.article>
  )
}
