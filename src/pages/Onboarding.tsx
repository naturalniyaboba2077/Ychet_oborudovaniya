/**
 * Шаг между аккаунтом и работой: у человека есть учётная запись, но он ещё
 * никуда не входит.
 *
 * Раньше регистрация создавала организацию заодно, и этого экрана не
 * существовало. Разделение понадобилось, чтобы вход через Google вообще стал
 * возможен: Google не сообщает ни телефона, ни названия компании, а
 * требовать их до входа — значит не дать войти.
 *
 * Экран показывается и тому, кто уже состоит в группах: со второй
 * организацией он попадает сюда сам, через «Добавить организацию».
 */
import { useState } from 'react'
import { useNavigate } from 'react-router'
import { Building2, Loader2, QrCode } from 'lucide-react'
import QrScanner from '@/components/QrScanner'
import { toast } from 'sonner'
import { trpc } from '@/providers/trpc'
import { cn } from '@/lib/utils'

const TIMEZONES = [
  'Europe/Kaliningrad',
  'Europe/Moscow',
  'Europe/Samara',
  'Asia/Yekaterinburg',
  'Asia/Novosibirsk',
  'Asia/Vladivostok',
] as const

const TIMEZONE_LABELS: Record<string, string> = {
  'Europe/Kaliningrad': 'Калининград, UTC+2',
  'Europe/Moscow': 'Москва, UTC+3',
  'Europe/Samara': 'Самара, UTC+4',
  'Asia/Yekaterinburg': 'Екатеринбург, UTC+5',
  'Asia/Novosibirsk': 'Новосибирск, UTC+7',
  'Asia/Vladivostok': 'Владивосток, UTC+10',
}

const inputCls =
  'h-11 w-full rounded-xl border border-brand-100 bg-surface px-3 text-sm text-ink-900 outline-none transition-shadow focus:border-brand-600 focus:ring-[3px] focus:ring-brand-600/15'

export default function Onboarding() {
  const navigate = useNavigate()
  const utils = trpc.useUtils()
  const wsQ = trpc.meta.workspaces.useQuery(undefined, { retry: 0 })
  const hasGroups = (wsQ.data?.length ?? 0) > 0

  const [mode, setMode] = useState<'choose' | 'create' | 'join'>('choose')
  const [name, setName] = useState('')
  const [timezone, setTimezone] = useState('Europe/Moscow')
  const [token, setToken] = useState('')

  /** Разбирает QR приглашения: и ссылку, и вложенный JSON. */
  const onScanned = (value: string) => {
    const trimmed = value.trim()
    try {
      const parsed = JSON.parse(trimmed) as { t?: string; token?: string }
      if (parsed.t === 'join' && parsed.token) {
        setToken(parsed.token)
        join.mutate({ token: parsed.token })
        return
      }
    } catch {
      /* не JSON — ниже попробуем как ссылку */
    }
    const match = trimmed.match(/[?&]token=([^&]+)/)
    if (match) {
      const found = decodeURIComponent(match[1])
      setToken(found)
      join.mutate({ token: found })
      return
    }
    toast.error('Это не QR-приглашение в группу')
  }

  const create = trpc.auth.createWorkspace.useMutation({
    onSuccess: async () => {
      await utils.invalidate()
      toast.success('Организация создана')
      navigate('/')
    },
    onError: (e) => toast.error(e.message || 'Не удалось создать организацию'),
  })

  const join = trpc.auth.join.useMutation({
    onSuccess: async () => {
      await utils.invalidate()
      toast.success('Вы вступили в организацию')
      navigate('/')
    },
    onError: (e) => toast.error(e.message || 'Не удалось вступить'),
  })

  return (
    <div className="min-h-[100dvh] bg-app flex items-center justify-center px-4 py-10">
      <div className="w-full max-w-lg">
        <h1 className="text-2xl font-bold text-ink-900">
          {hasGroups ? 'Добавить организацию' : 'Последний шаг'}
        </h1>
        <p className="mt-2 text-sm leading-6 text-ink-500">
          {hasGroups
            ? 'В одном аккаунте можно состоять в нескольких организациях — роль в каждой своя.'
            : 'Аккаунт готов. Теперь создайте свою организацию или вступите в чужую по приглашению.'}
        </p>

        {mode === 'choose' && (
          <div className="mt-6 grid gap-3">
            <button
              type="button"
              onClick={() => setMode('create')}
              className="flex items-start gap-3 rounded-2xl border border-brand-100 bg-white p-4 text-left transition-colors hover:bg-brand-50"
            >
              <Building2 size={20} className="mt-0.5 shrink-0 text-brand-600" />
              <span>
                <span className="block text-sm font-semibold text-ink-900">
                  Создать организацию
                </span>
                <span className="mt-0.5 block text-[13px] leading-5 text-ink-500">
                  Вы станете её владельцем и сможете приглашать остальных
                </span>
              </span>
            </button>

            <button
              type="button"
              onClick={() => setMode('join')}
              className="flex items-start gap-3 rounded-2xl border border-brand-100 bg-white p-4 text-left transition-colors hover:bg-brand-50"
            >
              <QrCode size={20} className="mt-0.5 shrink-0 text-brand-600" />
              <span>
                <span className="block text-sm font-semibold text-ink-900">
                  Вступить по приглашению
                </span>
                <span className="mt-0.5 block text-[13px] leading-5 text-ink-500">
                  Нужен код или QR от администратора — роль он задаёт сам
                </span>
              </span>
            </button>

            {hasGroups && (
              <button
                type="button"
                onClick={() => navigate('/')}
                className="mt-1 text-sm font-semibold text-brand-600 hover:text-brand-700"
              >
                Вернуться к работе
              </button>
            )}
          </div>
        )}

        {mode === 'create' && (
          <form
            className="mt-6 space-y-3"
            onSubmit={(e) => {
              e.preventDefault()
              if (name.trim().length < 2) {
                toast.error('Введите название организации')
                return
              }
              create.mutate({ name: name.trim(), timezone })
            }}
          >
            <div>
              <label className="mb-1.5 block text-[13px] font-semibold text-ink-900">
                Название организации
              </label>
              <input
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="ООО «СтройМонтаж»"
                className={inputCls}
              />
              <p className="mt-1 text-xs text-ink-500">Компания или бригада. Сменить можно позже</p>
            </div>

            <div>
              <label className="mb-1.5 block text-[13px] font-semibold text-ink-900">
                Часовой пояс
              </label>
              <select
                value={timezone}
                onChange={(e) => setTimezone(e.target.value)}
                className={inputCls}
              >
                {TIMEZONES.map((tz) => (
                  <option key={tz} value={tz}>
                    {TIMEZONE_LABELS[tz]}
                  </option>
                ))}
              </select>
            </div>

            <button
              disabled={create.isPending}
              className={cn(
                'flex h-12 w-full items-center justify-center gap-2 rounded-xl bg-accent text-sm font-semibold text-white transition-colors hover:bg-accent-hover',
                create.isPending && 'opacity-60',
              )}
            >
              {create.isPending && <Loader2 size={18} className="animate-spin" />}
              Создать
            </button>
            <button
              type="button"
              onClick={() => setMode('choose')}
              className="w-full text-sm font-semibold text-brand-600"
            >
              Назад
            </button>
          </form>
        )}

        {mode === 'join' && (
          <form
            className="mt-6 space-y-3"
            onSubmit={(e) => {
              e.preventDefault()
              const clean = token.trim()
              if (clean.length < 8) {
                toast.error('Введите код приглашения')
                return
              }
              join.mutate({ token: clean })
            }}
          >
            <div>
              <label className="mb-1.5 block text-[13px] font-semibold text-ink-900">
                Код приглашения
              </label>
              <input
                autoFocus
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="Код из QR администратора"
                className={inputCls}
              />
              <p className="mt-1 text-xs text-ink-500">
                Если у вас ссылка с QR — просто откройте её, код подставится сам
              </p>
            </div>

            {/* Сканер живёт здесь, а не на экране входа: вступают в группу
                уже войдя, иначе получалось бы, что приглашение заодно заводит
                учётную запись — именно от этого мы и ушли. */}
            <div>
              <p className="mb-2 text-[13px] font-semibold text-ink-900">
                Или наведите камеру на QR
              </p>
              <QrScanner onCode={onScanned} />
            </div>
            <button
              disabled={join.isPending}
              className={cn(
                'flex h-12 w-full items-center justify-center gap-2 rounded-xl bg-accent text-sm font-semibold text-white transition-colors hover:bg-accent-hover',
                join.isPending && 'opacity-60',
              )}
            >
              {join.isPending && <Loader2 size={18} className="animate-spin" />}
              Вступить
            </button>
            <button
              type="button"
              onClick={() => setMode('choose')}
              className="w-full text-sm font-semibold text-brand-600"
            >
              Назад
            </button>
          </form>
        )}
      </div>
    </div>
  )
}
