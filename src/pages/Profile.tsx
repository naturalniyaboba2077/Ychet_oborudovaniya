import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router'
import { AnimatePresence, motion } from 'framer-motion'
import {
  Camera,
  LogOut,
  Lock,
  Eye,
  EyeOff,
  AlertTriangle,
  Plus,
  UserPlus,
  Check,
  ChevronDown,
  Loader2,
  X,
} from 'lucide-react'
import { format } from 'date-fns'
import { cn } from '@/lib/utils'
import { trpc } from '@/providers/trpc'
import { useStore } from '@/lib/store'
import type { Rights } from '@/lib/rights'
import { inviteRoleLabel, parseInviteToken } from '@/lib/invite'
import QrScanner from '@/components/QrScanner'
// mock-данные только как запасной экран, если API ещё не ответил
import type { inferRouterOutputs } from '@trpc/server'
import type { AppRouter } from '../../api/router'

// ─── Типы ────────────────────────────────────────────────────────────────────

type RouterOutputs = inferRouterOutputs<AppRouter>
type ApiProfile = RouterOutputs['profile']['get']

interface VWorkspace {
  id: string
  name: string
  prefix: string
  timezone: string
  rights: Rights | null
}

interface VProfile {
  name: string
  position: string
  phone: string
  avatar: string
  createdAt: Date | null
  workspaces: VWorkspace[]
}

function adaptProfile(p: ApiProfile): VProfile {
  return {
    name: p.fullName,
    position: p.position ?? '',
    phone: p.phone,
    avatar: p.avatarUrl ?? '/avatar-1.png',
    createdAt: p.createdAt ? new Date(p.createdAt) : null,
    workspaces: p.workspaces.map((w) => ({
      id: String(w.id),
      name: w.name,
      prefix: w.internalIdPrefix,
      timezone: w.timezone,
      rights: (w.rights ?? null) as Rights | null,
    })),
  }
}

/**
 * Как назвать роль человека в организации.
 *
 * Раньше это выводилось из названия: «СтройМонтаж» значило «Владелец», а
 * рядом приписывалось «142 ед.» — число, взятое из воздуха. Человек видел
 * чужую роль и выдуманный остаток. Теперь роль читается из настоящих прав,
 * а придуманных чисел нет вовсе.
 */
function workspaceRole(rights: Rights | null | undefined): string {
  if (!rights) return 'Участник'
  if (rights.manageWorkspaces) return 'Создатель'
  if (rights.manageUsers) return 'Администратор'
  if (rights.writeOff || rights.replenish || rights.inventory) return 'Кладовщик'
  if (rights.createItems || rights.editItems) return 'Мастер'
  if (rights.transferItems) return 'Работник'
  return 'Наблюдатель'
}

// ─── Утилиты ─────────────────────────────────────────────────────────────────

type PwdScore = 0 | 1 | 2 | 3

function passwordScore(p: string): PwdScore {
  if (!p) return 0
  let s = 0
  if (p.length >= 8) s += 1
  if (/\d/.test(p) && /[a-zA-Zа-яА-ЯёЁ]/.test(p)) s += 1
  if (p.length >= 12 || /[^a-zA-Zа-яА-ЯёЁ0-9]/.test(p)) s += 1
  return Math.max(1, s) as PwdScore
}

const SCORE_COLOR: Record<PwdScore, string> = {
  0: '#C9C9F0',
  1: '#D64545',
  2: '#A87C0F',
  3: '#2E9E5B',
}

const SCORE_LABEL: Record<PwdScore, string> = {
  0: 'Минимум 8 символов, цифра и буква',
  1: 'Слабый пароль',
  2: 'Средний пароль',
  3: 'Надёжный пароль',
}

/** Сжимает выбранное фото до квадрата 256px (JPEG data URL) */
function fileToAvatarDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file)
    const img = new Image()
    img.onload = () => {
      const size = 256
      const canvas = document.createElement('canvas')
      canvas.width = size
      canvas.height = size
      const ctx = canvas.getContext('2d')
      if (!ctx) {
        URL.revokeObjectURL(url)
        reject(new Error('canvas недоступен'))
        return
      }
      const min = Math.min(img.width, img.height)
      ctx.drawImage(img, (img.width - min) / 2, (img.height - min) / 2, min, min, 0, 0, size, size)
      URL.revokeObjectURL(url)
      resolve(canvas.toDataURL('image/jpeg', 0.85))
    }
    img.onerror = () => {
      URL.revokeObjectURL(url)
      reject(new Error('Не удалось прочитать изображение'))
    }
    img.src = url
  })
}

// ─── UI-атомы ────────────────────────────────────────────────────────────────

function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string }) {
  return (
    <button type="button" role="switch" aria-checked={checked} onClick={() => onChange(!checked)} className="flex w-full items-center justify-between gap-4 py-2.5 text-left">
      <span>
        <span className="block text-sm font-semibold text-ink-900">{label}</span>
        {hint && <span className="block text-xs text-ink-500 mt-0.5">{hint}</span>}
      </span>
      <span
        className={cn(
          'relative w-11 h-6 rounded-full transition-colors duration-200 shrink-0',
          checked ? 'bg-brand-600' : 'bg-brand-100'
        )}
      >
        <motion.span
          initial={false}
          animate={{ x: checked ? 20 : 0 }}
          transition={{ type: 'spring', stiffness: 500, damping: 32 }}
          className="absolute left-0.5 top-0.5 block w-5 h-5 rounded-full bg-white shadow-card"
        />
      </span>
    </button>
  )
}

function Modal({
  open,
  onClose,
  title,
  danger,
  children,
}: {
  open: boolean
  onClose: () => void
  title: string
  danger?: boolean
  children: React.ReactNode
}) {
  return (
    <AnimatePresence>
      {open && (
        <div className="fixed inset-0 z-[80] flex items-end sm:items-center justify-center p-0 sm:p-4">
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.18 }}
            onClick={onClose}
            className="absolute inset-0 bg-[rgba(48,52,102,0.45)] backdrop-blur-[4px]"
          />
          <motion.div
            initial={{ opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.97, y: 8 }}
            transition={{ type: 'spring', stiffness: 300, damping: 26 }}
            className="relative w-full sm:max-w-[560px] bg-surface rounded-t-modal sm:rounded-modal shadow-modal p-6"
          >
            <div className="flex items-start justify-between gap-4">
              <div className="flex items-center gap-3">
                {danger && (
                  <motion.span
                    animate={{ scale: [1, 1.12, 1] }}
                    transition={{ duration: 1.2, repeat: Infinity }}
                    className="w-10 h-10 rounded-full bg-danger-bg text-danger flex items-center justify-center shrink-0"
                  >
                    <AlertTriangle size={18} />
                  </motion.span>
                )}
                <h3 className={cn('text-[17px] leading-6 font-semibold', danger ? 'text-danger' : 'text-ink-900')}>{title}</h3>
              </div>
              <button
                onClick={onClose}
                aria-label="Закрыть"
                className="w-8 h-8 rounded-lg text-ink-500 hover:bg-brand-50 flex items-center justify-center transition-colors"
              >
                <X size={16} />
              </button>
            </div>
            <div className="mt-4">{children}</div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  )
}

function Toast({ text, danger }: { text: string | null; danger?: boolean }) {
  return (
    <AnimatePresence>
      {text && (
        <motion.div
          initial={{ opacity: 0, y: 24 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: 12 }}
          transition={{ duration: 0.22, ease: 'easeOut' }}
          className="fixed bottom-20 lg:bottom-6 left-1/2 -translate-x-1/2 z-[90] flex items-center gap-2 rounded-full bg-ink-900 text-white text-sm font-semibold px-5 py-2.5 shadow-modal"
        >
          {danger ? <AlertTriangle size={16} className="text-danger" /> : <Check size={16} className="text-teal" />}
          {text}
        </motion.div>
      )}
    </AnimatePresence>
  )
}

function Card({ children, className, danger }: { children: React.ReactNode; className?: string; danger?: boolean }) {
  return (
    <section
      className={cn(
        'bg-surface rounded-card shadow-card p-5 lg:p-6',
        danger ? 'border-[1.5px] border-danger bg-[#FAD8D133]' : 'border border-brand-100/60',
        className
      )}
    >
      {children}
    </section>
  )
}

const inputCls =
  'h-11 w-full rounded-xl border border-brand-100 bg-white px-3.5 text-sm text-ink-900 placeholder:text-ink-300 focus:border-brand-600 focus:shadow-[0_0_0_3px_#5E629B22] transition-shadow disabled:bg-brand-50/60 disabled:text-ink-500'

const labelCls = 'block text-[13px] font-semibold text-ink-500 mb-1.5'

// ─── Страница ────────────────────────────────────────────────────────────────

export default function Profile() {
  const navigate = useNavigate()
  const { workspace, setWorkspace } = useStore()
  const utils = trpc.useUtils()

  const profileQ = trpc.profile.get.useQuery(undefined, { retry: 1 })

  // Привязка Google к уже существующей учётной записи.
  //
  // Отказ при регистрации с занятым телефоном отправлял человека сюда —
  // а кнопки здесь не было. Сама привязка безопасна именно потому, что
  // идёт из-под живой сессии: это и есть доказательство, что аккаунт твой.
  const [linkBusy, setLinkBusy] = useState(false)
  const googleLinked = profileQ.data?.googleLinked === true
  const googleEnabled =
    trpc.auth.options.useQuery(undefined, { retry: 0 }).data?.googleEnabled === true
  const linkGoogle = trpc.auth.googleBegin.useMutation({
    onSuccess: (data) => {
      // assign, а не присваивание href: правило линтера запрещает менять
      // свойства location напрямую.
      window.location.assign(data.url)
    },
    onError: (e) => {
      setLinkBusy(false)
      setToast({ text: e.message || 'Не удалось начать привязку Google', danger: true })
    },
  })
  // Счётчики над кнопкой выхода. Раньше здесь стояли литералы '12' и '3',
  // из-за чего любой человек — даже только что заведённый владелец пустой
  // базы — видел чужую выдумку вместо своих цифр.
  // limit: 1 — нужен только total, строки не нужны.
  const mineCountQ = trpc.items.list.useQuery(
    { onlyMine: true, page: 1, limit: 1 },
    { retry: 0 },
  )
  const transferCountsQ = trpc.meta.transferCounts.useQuery(undefined, { retry: 0 })
  const mineCount = mineCountQ.data?.total
  const transferCount =
    transferCountsQ.data === undefined
      ? undefined
      : transferCountsQ.data.incoming + transferCountsQ.data.outgoing

  // Профиль — это учётная запись, а не витрина: при ошибке показываем ошибку,
  // а не выдуманного пользователя.
  const profile: VProfile | null = useMemo(
    () => (profileQ.data ? adaptProfile(profileQ.data) : null),
    [profileQ.data],
  )

  // Покинутые прячем сразу, не дожидаясь ответа списка: сервер уже знает,
  // а перерисовка экрана идёт следом.
  const [leftWsIds, setLeftWsIds] = useState<Set<string>>(new Set())
  const [avatarOverride, setAvatarOverride] = useState<string | null>(null)

  const allWorkspaces: VWorkspace[] = useMemo(() => {
    const base = profile?.workspaces ?? []
    const merged = [...base]

    return merged.filter((w) => !leftWsIds.has(w.id))
  }, [profile, leftWsIds])

  // ─── Мутации ───────────────────────────────────────────────────────────────

  const [toast, setToast] = useState<{ text: string; danger?: boolean } | null>(null)
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const showToast = (text: string, danger = false) => {
    setToast({ text, danger })
    if (toastTimer.current) clearTimeout(toastTimer.current)
    toastTimer.current = setTimeout(() => setToast(null), 4000)
  }
  useEffect(() => () => {
    if (toastTimer.current) clearTimeout(toastTimer.current)
  }, [])

  const updateM = trpc.profile.update.useMutation({
    onSuccess: () => {
      utils.profile.get.invalidate()
      utils.meta.currentUser.invalidate()
    },
  })

  const pwdM = trpc.profile.changePassword.useMutation()

  const leaveWsM = trpc.profile.leaveWorkspace.useMutation({
    onSuccess: () => {
      utils.profile.get.invalidate()
      utils.meta.workspaces.invalidate()
      utils.items.list.invalidate()
    },
  })

  const deleteAccountM = trpc.profile.deleteAccount.useMutation()

  const joinWsM = trpc.auth.join.useMutation({
    onSuccess: () => {
      utils.profile.get.invalidate()
      utils.meta.workspaces.invalidate()
    },
  })

  const createWsM = trpc.admin.workspaces.create.useMutation({
    onSuccess: () => {
      utils.profile.get.invalidate()
      utils.meta.workspaces.invalidate()
    },
  })

  // ─── Состояние форм ────────────────────────────────────────────────────────

  const [fullName, setFullName] = useState('')
  const [position, setPosition] = useState('')
  const [flash, setFlash] = useState(false)
  useEffect(() => {
    if (!profile) return
    const frame = requestAnimationFrame(() => {
      setFullName(profile.name)
      setPosition(profile.position)
    })
    return () => cancelAnimationFrame(frame)
  }, [profile])

  const [pwdCurrent, setPwdCurrent] = useState('')
  const [pwdNew, setPwdNew] = useState('')
  const [pwdRepeat, setPwdRepeat] = useState('')
  const [showCurrent, setShowCurrent] = useState(false)
  const [showNew, setShowNew] = useState(false)
  const [pwdError, setPwdError] = useState<string | null>(null)
  const [shakeKey, setShakeKey] = useState(0)

  const [settings, setSettings] = useState({
    pushTransfers: true,
    remindTo: true,
    weeklyEmail: false,
    compactCatalog: false,
  })

  const [logoutOpen, setLogoutOpen] = useState(false)
  const [createWsOpen, setCreateWsOpen] = useState(false)
  const [joinWsOpen, setJoinWsOpen] = useState(false)
  const [joinLink, setJoinLink] = useState('')
  const [joinScan, setJoinScan] = useState(false)
  const joinToken = parseInviteToken(joinLink)
  // До вступления показываем, куда и кем человек войдёт: приглашение может
  // быть и от чужой организации, и на роль администратора.
  const joinInfo = trpc.auth.inviteInfo.useQuery(
    { token: joinToken },
    { enabled: joinWsOpen && joinToken.length >= 8, retry: false }
  )
  const [wsName, setWsName] = useState('')
  const [wsTz, setWsTz] = useState('Europe/Moscow')
  const [wsPrefix, setWsPrefix] = useState('ВН-')
  const [leaveWs, setLeaveWs] = useState<VWorkspace | null>(null)
  const [leaveConfirm, setLeaveConfirm] = useState('')
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [deleteAgree, setDeleteAgree] = useState(false)
  const [deletePwd, setDeletePwd] = useState('')

  const fileRef = useRef<HTMLInputElement>(null)

  // ─── Обработчики ───────────────────────────────────────────────────────────

  const onAvatarFile = async (file: File) => {
    try {
      const dataUrl = await fileToAvatarDataUrl(file)
      setAvatarOverride(dataUrl)
      updateM.mutate(
        { avatarUrl: dataUrl },
        {
          onSuccess: () => showToast('Фото обновлено'),
          // Показывать «сохранено», когда сервер отказал, нельзя: человек
          // уходит уверенным, что фото на месте, а его там нет.
          onError: (e) => {
            setAvatarOverride(null)
            showToast(e.message || 'Не удалось сохранить фото', true)
          },
        }
      )
    } catch {
      showToast('Не удалось загрузить фото', true)
    }
  }

  const onSavePersonal = () => {
    if (!fullName.trim()) {
      showToast('Укажите имя и фамилию', true)
      return
    }
    updateM.mutate(
      { fullName: fullName.trim(), position: position.trim() || null },
      {
        onSuccess: () => showToast('Данные обновлены'),
        onError: (e) => showToast(e.message || 'Не удалось сохранить данные', true),
      }
    )
    setFlash(true)
    setTimeout(() => setFlash(false), 900)
  }

  const score = passwordScore(pwdNew)

  const onChangePassword = () => {
    if (!pwdCurrent) {
      setPwdError('Введите текущий пароль')
      return
    }
    if (pwdNew.length < 8 || !/\d/.test(pwdNew) || !/[a-zA-Zа-яА-ЯёЁ]/.test(pwdNew)) {
      setPwdError('Новый пароль: минимум 8 символов, цифра и буква')
      return
    }
    if (pwdNew !== pwdRepeat) {
      setPwdError('Пароли не совпадают')
      setShakeKey((k) => k + 1)
      return
    }
    setPwdError(null)
    pwdM.mutate(
      { currentPassword: pwdCurrent, newPassword: pwdNew },
      {
        onSuccess: () => {
          showToast('Пароль изменён')
          setPwdCurrent('')
          setPwdNew('')
          setPwdRepeat('')
        },
        onError: (e) => showToast(e.message || 'Не удалось изменить пароль', true),
      }
    )
  }

  const onSwitchWorkspace = (ws: VWorkspace) => {
    const workspace = {
      id: Number(ws.id),
      name: ws.name,
      internalIdPrefix: ws.prefix,
    }
    setWorkspace(workspace)
    showToast(`Переключено на ${ws.name}`)
  }

  const onCreateWorkspace = () => {
    const name = wsName.trim()
    if (!name) {
      showToast('Укажите название пространства', true)
      return
    }
    createWsM.mutate(
      { name, timezone: wsTz, internalIdPrefix: wsPrefix.trim() || 'ВН-' },
      {
        onSuccess: () => {
          showToast('Рабочее пространство создано')
        },
        // Раньше на отказ сервера в список подставлялась выдуманная
        // запись: организация была видна, но не существовала.
        onError: (e) => showToast(e.message || 'Не удалось создать пространство', true),
      }
    )
    setCreateWsOpen(false)
    setWsName('')
    setWsPrefix('ВН-')
  }

  // Принимает ссылку-приглашение, код из неё и QR: ссылку пересылают в
  // мессенджере, код диктуют, QR показывают с экрана администратора.
  const onJoinWorkspace = (token = joinToken) => {
    if (!token) {
      showToast('Вставьте ссылку-приглашение или код из неё', true)
      return
    }
    joinWsM.mutate(
      { token },
      {
        onSuccess: (ws) => {
          showToast(ws?.name ? `Вы присоединились к «${ws.name}»` : 'Вы присоединились к пространству')
          // Вступали, чтобы работать там, — сразу туда и переключаем.
          if (ws?.id) {
            setWorkspace({ id: Number(ws.id), name: ws.name, internalIdPrefix: ws.internalIdPrefix })
          }
          closeJoin()
        },
        onError: (e) => showToast(e.message || 'Не удалось присоединиться', true),
      }
    )
  }

  const closeJoin = () => {
    setJoinWsOpen(false)
    setJoinScan(false)
    setJoinLink('')
  }

  const onJoinScanned = (code: string) => {
    const token = parseInviteToken(code)
    if (!token) {
      showToast('Это не QR-приглашение в организацию', true)
      return
    }
    setJoinScan(false)
    setJoinLink(token)
  }

  const onLeaveWorkspace = () => {
    if (!leaveWs || leaveConfirm.trim() !== leaveWs.name) return
    const id = Number(leaveWs.id)
    if (!Number.isFinite(id)) return
    const name = leaveWs.name
    leaveWsM.mutate(
      { workspaceId: id },
      {
        onSuccess: (res) => {
          setLeftWsIds((prev) => new Set(prev).add(leaveWs.id))
          showToast(
            res.releasedItems > 0
              ? `Вы покинули ${name}; инструмент (${res.releasedItems}) вернулся на склад`
              : `Вы покинули ${name}`,
          )
          setLeaveWs(null)
          setLeaveConfirm('')
        },
        // Сервер отказывает по делу — например, если это последний
        // руководитель. Прятать такой отказ нельзя.
        onError: (e) => showToast(e.message || 'Не удалось выйти', true),
      },
    )
  }

  const onDeleteAccount = () => {
    if (!deleteAgree || !deletePwd) return
    deleteAccountM.mutate(
      { password: deletePwd },
      {
        onSuccess: () => {
          setDeleteOpen(false)
          setDeleteAgree(false)
          setDeletePwd('')
          showToast('Аккаунт удалён')
          setTimeout(() => window.location.assign('/login'), 600)
        },
        // Неверный пароль, последняя организация без руководителя — всё
        // это причины, по которым удаление не состоялось, и человек
        // должен их увидеть, а не оказаться на экране входа.
        onError: (e) => {
          setDeletePwd('')
          showToast(e.message || 'Не удалось удалить аккаунт', true)
        },
      },
    )
  }

  // ─── Render ────────────────────────────────────────────────────────────────

  if (!profile && profileQ.isError) {
    return (
      <div className="rounded-card border border-danger/30 bg-danger/5 p-5 text-sm text-danger">
        <p className="font-semibold">Профиль не загружен.</p>
        <p className="mt-1">{profileQ.error.message}</p>
        <button
          type="button"
          onClick={() => void profileQ.refetch()}
          className="mt-3 inline-flex h-10 items-center rounded-xl bg-accent px-4 text-sm font-semibold text-white hover:bg-accent-hover"
        >
          Повторить
        </button>
      </div>
    )
  }

  if (!profile) {
    return (
      <div className="grid lg:grid-cols-[320px_1fr] gap-5">
        <div className="bg-surface rounded-card border border-brand-100/60 shadow-card h-[420px] animate-skeleton-pulse" />
        <div className="space-y-5">
          <div className="bg-surface rounded-card border border-brand-100/60 shadow-card h-44 animate-skeleton-pulse" />
          <div className="bg-surface rounded-card border border-brand-100/60 shadow-card h-44 animate-skeleton-pulse" />
        </div>
      </div>
    )
  }

  const avatarSrc = avatarOverride ?? profile.avatar

  return (
    <div className="space-y-5">
      <motion.h1
        initial={{ opacity: 0, y: 16 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.28 }}
        className="text-2xl lg:text-[28px] leading-9 font-bold tracking-[-0.01em] text-ink-900"
      >
        Мой профиль
      </motion.h1>

      <div className="grid lg:grid-cols-[320px_1fr] gap-5 items-start">
        {/* ─── Секция 1. Визитка ─── */}
        <Card className="lg:sticky lg:top-20 text-center p-6 lg:p-8">
          <motion.div
            initial={{ scale: 0.9, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            transition={{ type: 'spring', stiffness: 260, damping: 20 }}
            className="relative inline-block"
          >
            <img
              src={avatarSrc}
              alt={profile.name}
              className="w-20 h-20 lg:w-[120px] lg:h-[120px] rounded-full object-cover border-2 border-brand-100"
            />
            <button
              onClick={() => fileRef.current?.click()}
              aria-label="Сменить фото"
              className="absolute right-0 bottom-0 w-9 h-9 rounded-full bg-brand-600 text-white flex items-center justify-center shadow-card hover:bg-brand-700 active:scale-95 transition"
            >
              <Camera size={16} />
            </button>
            <input
              ref={fileRef}
              type="file"
              accept="image/*"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0]
                if (f) void onAvatarFile(f)
                e.target.value = ''
              }}
            />
          </motion.div>

          <motion.div
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.25, delay: 0.05 }}
          >
            <h2 className="mt-4 text-xl leading-7 font-semibold text-ink-900">{profile.name}</h2>
            <p className="text-sm text-ink-500 mt-0.5">{profile.position || '—'}</p>
            <p className="mt-3 font-mono-num text-ink-900">{profile.phone}</p>
            <span className="mt-2 inline-flex items-center gap-1.5 rounded-full bg-brand-50 px-2.5 py-1 text-[11px] font-semibold text-ink-500">
              <Lock size={11} />
              логин = телефон
            </span>
          </motion.div>

          <motion.div
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.25, delay: 0.1 }}
            className="mt-5 grid grid-cols-3 divide-x divide-brand-100/70"
          >
            {[
              { v: mineCount === undefined ? '—' : String(mineCount), l: 'ед. на мне' },
              {
                v: transferCount === undefined ? '—' : String(transferCount),
                l: 'передачи',
              },
              { v: profile.createdAt ? format(profile.createdAt, 'dd.MM.yy') : '—', l: 'в сервисе с' },
            ].map((s) => (
              <div key={s.l} className="px-1">
                <div className="text-xl font-bold text-ink-900">{s.v}</div>
                <div className="text-[11px] text-ink-500 mt-0.5 leading-tight">{s.l}</div>
              </div>
            ))}
          </motion.div>

          {googleEnabled && !googleLinked && (
            <button
              type="button"
              disabled={linkBusy}
              onClick={() => {
                setLinkBusy(true)
                linkGoogle.mutate({})
              }}
              className="mt-6 inline-flex w-full items-center justify-center gap-2 h-10 rounded-xl border border-brand-100 bg-white text-sm font-semibold text-ink-900 hover:bg-brand-50 transition disabled:opacity-60"
            >
              {linkBusy ? (
                <Loader2 size={16} className="animate-spin" />
              ) : (
                <svg width="16" height="16" viewBox="0 0 48 48" aria-hidden="true">
                  <path fill="#EA4335" d="M24 9.5c3.5 0 6.6 1.2 9 3.6l6.7-6.7C35.6 2.6 30.2 0 24 0 14.6 0 6.5 5.4 2.6 13.2l7.8 6.1C12.3 13.2 17.7 9.5 24 9.5z" />
                  <path fill="#4285F4" d="M46.1 24.6c0-1.6-.1-3.1-.4-4.6H24v9.1h12.4c-.5 2.9-2.2 5.3-4.7 7l7.6 5.9c4.4-4.1 6.8-10.2 6.8-17.4z" />
                  <path fill="#FBBC05" d="M10.4 28.7c-.5-1.4-.8-2.9-.8-4.7s.3-3.3.8-4.7l-7.8-6.1C.9 16.5 0 20.1 0 24s.9 7.5 2.6 10.8l7.8-6.1z" />
                  <path fill="#34A853" d="M24 48c6.5 0 11.9-2.1 15.9-5.8l-7.6-5.9c-2.1 1.4-4.8 2.3-8.3 2.3-6.3 0-11.7-3.7-13.6-9.8l-7.8 6.1C6.5 42.6 14.6 48 24 48z" />
                </svg>
              )}
              Привязать Google
            </button>
          )}
          {googleLinked && (
            <p className="mt-6 text-center text-[13px] text-ink-500">
              Google привязан{profileQ.data?.email ? `: ${profileQ.data.email}` : ''} — можно входить одним нажатием
            </p>
          )}

          <button
            onClick={() => setLogoutOpen(true)}
            className="mt-3 inline-flex w-full items-center justify-center gap-2 h-10 rounded-xl border border-brand-100 bg-white text-sm font-semibold text-ink-900 hover:bg-brand-50 transition"
          >
            <LogOut size={16} />
            Выйти из аккаунта
          </button>
        </Card>

        {/* ─── Правая колонка ─── */}
        <div className="space-y-5 min-w-0">
          {/* Секция 2. Личные данные */}
          <Card>
            <h3 className="text-[17px] leading-6 font-semibold text-ink-900 mb-4">Личные данные</h3>
            <div className={cn('grid sm:grid-cols-2 gap-4 rounded-xl transition-colors duration-500', flash && 'bg-success-bg/60')}>
              <label className="block">
                <span className={labelCls}>
                  Имя и фамилия <span className="text-accent">*</span>
                </span>
                <input value={fullName} onChange={(e) => setFullName(e.target.value)} className={inputCls} />
              </label>
              <label className="block">
                <span className={labelCls}>Должность</span>
                <input value={position} onChange={(e) => setPosition(e.target.value)} className={inputCls} />
              </label>
              <label className="block sm:col-span-2">
                <span className={labelCls}>Телефон</span>
                <div className="relative">
                  <input value={profile.phone} disabled className={cn(inputCls, 'font-mono-num pr-10')} />
                  <Lock size={14} className="absolute right-3.5 top-1/2 -translate-y-1/2 text-ink-300" />
                </div>
                <span className="block text-xs text-ink-500 mt-1.5">
                  Телефон — ваш логин. Изменение — через поддержку
                </span>
              </label>
            </div>
            <div className="mt-4">
              <button
                onClick={onSavePersonal}
                disabled={updateM.isPending}
                className="inline-flex items-center gap-2 h-10 px-5 rounded-xl bg-accent text-white text-sm font-semibold hover:bg-accent-hover hover:-translate-y-px active:scale-[0.97] transition disabled:opacity-70"
              >
                {updateM.isPending && <Loader2 size={16} className="animate-spin" />}
                Сохранить
              </button>
            </div>
          </Card>

          {/* Секция 3. Пароль */}
          <Card>
            <h3 className="text-[17px] leading-6 font-semibold text-ink-900 mb-4">Пароль</h3>
            <div className="grid sm:grid-cols-2 gap-4">
              <label className="block">
                <span className={labelCls}>Текущий пароль</span>
                <div className="relative">
                  <input
                    type={showCurrent ? 'text' : 'password'}
                    value={pwdCurrent}
                    onChange={(e) => setPwdCurrent(e.target.value)}
                    className={cn(inputCls, 'pr-10')}
                    autoComplete="current-password"
                  />
                  <button
                    type="button"
                    onClick={() => setShowCurrent((v) => !v)}
                    aria-label="Показать пароль"
                    className="absolute right-3 top-1/2 -translate-y-1/2 text-ink-300 hover:text-ink-500"
                  >
                    {showCurrent ? <EyeOff size={16} /> : <Eye size={16} />}
                  </button>
                </div>
              </label>
              <div className="hidden sm:block" />
              <label className="block">
                <span className={labelCls}>Новый пароль</span>
                <div className="relative">
                  <input
                    type={showNew ? 'text' : 'password'}
                    value={pwdNew}
                    onChange={(e) => setPwdNew(e.target.value)}
                    className={cn(inputCls, 'pr-10')}
                    autoComplete="new-password"
                  />
                  <button
                    type="button"
                    onClick={() => setShowNew((v) => !v)}
                    aria-label="Показать пароль"
                    className="absolute right-3 top-1/2 -translate-y-1/2 text-ink-300 hover:text-ink-500"
                  >
                    {showNew ? <EyeOff size={16} /> : <Eye size={16} />}
                  </button>
                </div>
                <div className="mt-2 h-1 rounded-full bg-brand-50 overflow-hidden">
                  <motion.div
                    initial={false}
                    animate={{
                      width: pwdNew ? `${Math.max(12, (score / 3) * 100)}%` : '0%',
                      backgroundColor: SCORE_COLOR[score],
                    }}
                    transition={{ duration: 0.2 }}
                    className="h-full rounded-full"
                  />
                </div>
                <span className="block text-xs text-ink-500 mt-1.5" style={pwdNew ? { color: SCORE_COLOR[score] } : undefined}>
                  {SCORE_LABEL[score]}
                </span>
              </label>
              <label className="block">
                <span className={labelCls}>Повторите новый пароль</span>
                <motion.div
                  key={shakeKey}
                  animate={shakeKey > 0 ? { x: [0, -6, 6, -4, 4, 0] } : undefined}
                  transition={{ duration: 0.4 }}
                >
                  <input
                    type="password"
                    value={pwdRepeat}
                    onChange={(e) => setPwdRepeat(e.target.value)}
                    className={cn(
                      inputCls,
                      pwdRepeat && pwdRepeat !== pwdNew && 'border-danger focus:border-danger focus:shadow-[0_0_0_3px_#D6454522]'
                    )}
                    autoComplete="new-password"
                  />
                </motion.div>
                {pwdRepeat && pwdRepeat !== pwdNew && (
                  <span className="block text-xs font-semibold text-danger mt-1.5">Пароли не совпадают</span>
                )}
              </label>
            </div>
            {pwdError && <p className="mt-3 text-xs font-semibold text-danger">{pwdError}</p>}
            <div className="mt-4">
              <button
                onClick={onChangePassword}
                disabled={pwdM.isPending}
                className="inline-flex items-center gap-2 h-10 px-5 rounded-xl border border-brand-100 bg-white text-sm font-semibold text-ink-900 hover:bg-brand-50 transition disabled:opacity-70"
              >
                {pwdM.isPending && <Loader2 size={16} className="animate-spin" />}
                Изменить пароль
              </button>
            </div>
          </Card>

          {/* Секция 4. Рабочие пространства */}
          <Card>
            <h3 className="text-[17px] leading-6 font-semibold text-ink-900 mb-4">Рабочие пространства</h3>
            <div className="space-y-2.5">
              <AnimatePresence initial={false}>
                {allWorkspaces.map((ws) => {
                  const isCurrent = Number(ws.id) === workspace?.id
                  const role = workspaceRole(ws.rights)
                  return (
                    <motion.div
                      key={ws.id}
                      layout="position"
                      initial={{ opacity: 0, y: 8 }}
                      animate={{ opacity: 1, y: 0 }}
                      exit={{ opacity: 0, y: -8 }}
                      transition={{ duration: 0.22 }}
                      className={cn(
                        'flex items-center gap-3 rounded-xl border px-4 py-3 transition-colors',
                        isCurrent ? 'border-brand-600/40 bg-brand-50/60' : 'border-brand-100/60 hover:bg-brand-50/40'
                      )}
                    >
                      <img src="/logo-mark.svg" alt="" className="w-10 h-10 rounded-lg shrink-0" />
                      <span className="min-w-0 flex-1">
                        <span className="block text-[15px] font-semibold text-ink-900 truncate">{ws.name}</span>
                        <span className="block text-xs text-ink-500">
                          {role}
                        </span>
                      </span>
                      <AnimatePresence>
                        {isCurrent && (
                          <motion.span
                            initial={{ opacity: 0, x: -8 }}
                            animate={{ opacity: 1, x: 0 }}
                            exit={{ opacity: 0 }}
                            className="shrink-0 rounded-full bg-teal/20 px-2.5 py-0.5 text-[11px] font-semibold text-teal-dark"
                          >
                            Текущее
                          </motion.span>
                        )}
                      </AnimatePresence>
                      {!isCurrent && (
                        <button
                          onClick={() => onSwitchWorkspace(ws)}
                          className="shrink-0 h-8 px-3.5 rounded-xl text-[13px] font-semibold text-brand-600 hover:bg-brand-50 transition-colors"
                        >
                          Переключиться
                        </button>
                      )}
                    </motion.div>
                  )
                })}
              </AnimatePresence>
            </div>
            <button
              onClick={() => setCreateWsOpen(true)}
              className="mt-4 inline-flex items-center gap-2 h-10 px-5 rounded-xl border border-brand-100 bg-white text-sm font-semibold text-ink-900 hover:bg-brand-50 transition"
            >
              <Plus size={16} />
              Создать своё пространство
            </button>
            <button
              onClick={() => setJoinWsOpen(true)}
              className="mt-4 ml-0 sm:ml-3 inline-flex items-center gap-2 h-10 px-5 rounded-xl border border-brand-100 bg-white text-sm font-semibold text-ink-900 hover:bg-brand-50 transition"
            >
              <UserPlus size={16} />
              Вступить в существующее
            </button>
            <p className="mt-3 text-xs leading-5 text-ink-500">
              В своём пространстве вы администратор. В чужое вступают по приглашению его
              администратора — обычно сотрудником, с теми правами, что выдал он.
            </p>
          </Card>

          {/* Секция 5. Настройки */}
          <Card>
            <h3 className="text-[17px] leading-6 font-semibold text-ink-900 mb-2">Настройки</h3>
            <div className="divide-y divide-brand-100/50">
              <Toggle
                checked={settings.pushTransfers}
                onChange={(v) => setSettings((s) => ({ ...s, pushTransfers: v }))}
                label="Push-уведомления о передачах"
              />
              <Toggle
                checked={settings.remindTo}
                onChange={(v) => setSettings((s) => ({ ...s, remindTo: v }))}
                label="Напоминания о ТО и поверках"
              />
              <Toggle
                checked={settings.weeklyEmail}
                onChange={(v) => setSettings((s) => ({ ...s, weeklyEmail: v }))}
                label="Сводка по итогам недели на e-mail"
              />
              <Toggle
                checked={settings.compactCatalog}
                onChange={(v) => setSettings((s) => ({ ...s, compactCatalog: v }))}
                label="Компактный вид каталога"
              />
            </div>
            <div className="mt-3">
              <span className={labelCls}>Язык интерфейса</span>
              <div className="relative max-w-xs">
                <select
                  defaultValue="ru"
                  className="appearance-none h-11 w-full rounded-xl border border-brand-100 bg-white pl-3.5 pr-10 text-sm font-semibold text-ink-900 focus:border-brand-600 cursor-pointer"
                >
                  <option value="ru">Русский</option>
                  <option value="en" disabled>
                    English — скоро
                  </option>
                </select>
                <ChevronDown size={16} className="pointer-events-none absolute right-3.5 top-1/2 -translate-y-1/2 text-ink-300" />
              </div>
            </div>
          </Card>

          {/* Секция 6. Опасная зона */}
          <Card danger>
            <h3 className="text-[17px] leading-6 font-semibold text-danger mb-4">Опасная зона</h3>
            <div className="space-y-4">
              {allWorkspaces
                .filter((ws) => ws.name !== workspace?.name)
                .map((ws) => (
                  <div key={ws.id} className="flex flex-wrap items-center justify-between gap-3">
                    <span className="text-sm font-semibold text-ink-900">
                      Покинуть рабочее пространство {ws.name}
                    </span>
                    <motion.button
                      whileHover={{ x: [0, -2, 2, -2, 0] }}
                      transition={{ duration: 0.3 }}
                      onClick={() => {
                        setLeaveWs(ws)
                        setLeaveConfirm('')
                      }}
                      className="h-9 px-4 rounded-xl border border-danger text-sm font-semibold text-danger hover:bg-danger-bg transition-colors"
                    >
                      Покинуть
                    </motion.button>
                  </div>
                ))}
              <div className="flex flex-wrap items-center justify-between gap-3 pt-4 border-t border-danger/20">
                <span className="max-w-md">
                  <span className="block text-sm font-semibold text-ink-900">Удалить аккаунт</span>
                  <span className="block text-xs text-ink-500 mt-0.5">
                    Удалятся профиль и доступы. История операций сохранится в журнале пространства обезличенно.
                  </span>
                </span>
                <motion.button
                  whileHover={{ x: [0, -2, 2, -2, 0] }}
                  transition={{ duration: 0.3 }}
                  onClick={() => setDeleteOpen(true)}
                  className="h-9 px-4 rounded-xl bg-danger text-white text-sm font-semibold hover:brightness-95 active:scale-[0.97] transition"
                >
                  Удалить аккаунт
                </motion.button>
              </div>
            </div>
          </Card>
        </div>
      </div>

      {/* ─── Модалки ─── */}

      {/* Выход из аккаунта */}
      <Modal open={logoutOpen} onClose={() => setLogoutOpen(false)} title="Выйти из аккаунта?">
        <p className="text-sm text-ink-500">
          Вы сможете войти снова по номеру телефона. Несохранённые изменения на открытых страницах будут потеряны.
        </p>
        <div className="mt-5 flex justify-end gap-3">
          <button
            onClick={() => setLogoutOpen(false)}
            className="h-10 px-4 rounded-xl text-sm font-semibold text-brand-600 hover:bg-brand-50 transition-colors"
          >
            Отмена
          </button>
          <button
            onClick={() => {
              setLogoutOpen(false)
              navigate('/login')
            }}
            className="h-10 px-5 rounded-xl bg-accent text-white text-sm font-semibold hover:bg-accent-hover active:scale-[0.97] transition"
          >
            Выйти
          </button>
        </div>
      </Modal>

      {/* Создание рабочего пространства */}
      <Modal open={createWsOpen} onClose={() => setCreateWsOpen(false)} title="Новое рабочее пространство">
        <div className="space-y-4">
          <label className="block">
            <span className={labelCls}>
              Название <span className="text-accent">*</span>
            </span>
            <input
              value={wsName}
              onChange={(e) => setWsName(e.target.value)}
              placeholder="ООО «Новая компания»"
              className={inputCls}
            />
          </label>
          <div className="grid sm:grid-cols-2 gap-4">
            <label className="block">
              <span className={labelCls}>Часовой пояс</span>
              <div className="relative">
                <select
                  value={wsTz}
                  onChange={(e) => setWsTz(e.target.value)}
                  className="appearance-none h-11 w-full rounded-xl border border-brand-100 bg-white pl-3.5 pr-10 text-sm text-ink-900 focus:border-brand-600 cursor-pointer"
                >
                  <option value="Europe/Kaliningrad">Калининград (UTC+2)</option>
                  <option value="Europe/Moscow">Москва (UTC+3)</option>
                  <option value="Europe/Samara">Самара (UTC+4)</option>
                  <option value="Asia/Yekaterinburg">Екатеринбург (UTC+5)</option>
                  <option value="Asia/Novosibirsk">Новосибирск (UTC+7)</option>
                  <option value="Asia/Vladivostok">Владивосток (UTC+10)</option>
                </select>
                <ChevronDown size={16} className="pointer-events-none absolute right-3.5 top-1/2 -translate-y-1/2 text-ink-300" />
              </div>
            </label>
            <label className="block">
              <span className={labelCls}>Префикс номеров</span>
              <input
                value={wsPrefix}
                onChange={(e) => setWsPrefix(e.target.value)}
                placeholder="ВН-"
                maxLength={8}
                className={cn(inputCls, 'font-mono-num')}
              />
            </label>
          </div>
        </div>
        <div className="mt-5 flex justify-end gap-3">
          <button
            onClick={() => setCreateWsOpen(false)}
            className="h-10 px-4 rounded-xl text-sm font-semibold text-brand-600 hover:bg-brand-50 transition-colors"
          >
            Отмена
          </button>
          <button
            onClick={onCreateWorkspace}
            disabled={createWsM.isPending}
            className="inline-flex items-center gap-2 h-10 px-5 rounded-xl bg-accent text-white text-sm font-semibold hover:bg-accent-hover active:scale-[0.97] transition disabled:opacity-70"
          >
            {createWsM.isPending && <Loader2 size={16} className="animate-spin" />}
            Создать
          </button>
        </div>
      </Modal>

      {/* Вступление в пространство по приглашению */}
      <Modal open={joinWsOpen} onClose={closeJoin} title="Вступить в пространство">
        <label className="block">
          <span className={labelCls}>
            Ссылка-приглашение или код <span className="text-accent">*</span>
          </span>
          <input
            value={joinLink}
            onChange={(e) => setJoinLink(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && onJoinWorkspace()}
            placeholder="https://…/join?token=…"
            autoFocus
            className={inputCls}
          />
        </label>
        {joinScan ? (
          <div className="mt-3">
            <QrScanner onCode={onJoinScanned} />
          </div>
        ) : (
          <button
            type="button"
            onClick={() => setJoinScan(true)}
            className="mt-2 text-sm font-semibold text-brand-600 hover:underline"
          >
            Отсканировать QR-приглашение
          </button>
        )}
        {joinInfo.data && (
          <div className="mt-3 rounded-xl border border-brand-100 bg-brand-50/60 px-4 py-3 text-sm">
            <span className="block font-semibold text-ink-900">{joinInfo.data.workspace?.name}</span>
            <span className="block text-ink-500">Вы войдёте как: {inviteRoleLabel(joinInfo.data.role)}</span>
          </div>
        )}
        {joinInfo.error && (
          <p className="mt-3 text-sm text-danger">{joinInfo.error.message}</p>
        )}
        <p className="mt-2 text-xs leading-5 text-ink-500">
          Приглашение выдаёт администратор организации. Права в ней будут свои — то, что
          вы можете в текущем пространстве, туда не переносится.
        </p>
        <div className="mt-5 flex justify-end gap-3">
          <button
            onClick={closeJoin}
            className="h-10 px-4 rounded-xl text-sm font-semibold text-brand-600 hover:bg-brand-50 transition-colors"
          >
            Отмена
          </button>
          <button
            onClick={() => onJoinWorkspace()}
            disabled={joinWsM.isPending || !joinToken || joinInfo.isError}
            className="inline-flex items-center gap-2 h-10 px-5 rounded-xl bg-accent text-white text-sm font-semibold hover:bg-accent-hover active:scale-[0.97] transition disabled:opacity-70"
          >
            {joinWsM.isPending && <Loader2 size={16} className="animate-spin" />}
            Присоединиться
          </button>
        </div>
      </Modal>

      {/* Покинуть пространство */}
      <Modal
        open={leaveWs !== null}
        onClose={() => setLeaveWs(null)}
        title={`Покинуть ${leaveWs?.name ?? ''}?`}
        danger
      >
        <p className="text-sm text-ink-500">
          Вы потеряете доступ к имуществу и журналу этого пространства. Для подтверждения введите название
          пространства: <span className="font-semibold text-ink-900">{leaveWs?.name}</span>
        </p>
        <input
          value={leaveConfirm}
          onChange={(e) => setLeaveConfirm(e.target.value)}
          placeholder={leaveWs?.name}
          className={cn(inputCls, 'mt-4')}
        />
        <div className="mt-5 flex justify-end gap-3">
          <button
            onClick={() => setLeaveWs(null)}
            className="h-10 px-4 rounded-xl text-sm font-semibold text-brand-600 hover:bg-brand-50 transition-colors"
          >
            Отмена
          </button>
          <button
            onClick={onLeaveWorkspace}
            disabled={!leaveWs || leaveConfirm.trim() !== leaveWs.name}
            className="h-10 px-5 rounded-xl bg-danger text-white text-sm font-semibold hover:brightness-95 active:scale-[0.97] transition disabled:opacity-50"
          >
            Покинуть пространство
          </button>
        </div>
      </Modal>

      {/* Удаление аккаунта */}
      <Modal open={deleteOpen} onClose={() => setDeleteOpen(false)} title="Удалить аккаунт?" danger>
        <p className="text-sm text-ink-500">
          Удалятся профиль и доступы. История операций сохранится в журнале пространства обезличенно.
        </p>
        <label className="mt-4 flex items-start gap-2.5 cursor-pointer select-none">
          <span
            className={cn(
              'mt-0.5 w-5 h-5 rounded-md border-[1.5px] flex items-center justify-center transition-colors shrink-0',
              deleteAgree ? 'bg-danger border-danger' : 'border-brand-100 bg-white'
            )}
            onClick={() => setDeleteAgree((v) => !v)}
          >
            {deleteAgree && <Check size={13} strokeWidth={3} className="text-white" />}
          </span>
          <span className="text-sm text-ink-900" onClick={() => setDeleteAgree((v) => !v)}>
            Я понимаю, что действие необратимо
          </span>
        </label>
        <label className="block mt-4">
          <span className={labelCls}>Пароль для подтверждения</span>
          <input
            type="password"
            value={deletePwd}
            onChange={(e) => setDeletePwd(e.target.value)}
            className={inputCls}
            autoComplete="current-password"
          />
        </label>
        <div className="mt-5 flex justify-end gap-3">
          <button
            onClick={() => setDeleteOpen(false)}
            className="h-10 px-4 rounded-xl text-sm font-semibold text-brand-600 hover:bg-brand-50 transition-colors"
          >
            Отмена
          </button>
          <button
            onClick={onDeleteAccount}
            disabled={!deleteAgree || !deletePwd}
            className="h-10 px-5 rounded-xl bg-danger text-white text-sm font-semibold hover:brightness-95 active:scale-[0.97] transition disabled:opacity-50"
          >
            Удалить навсегда
          </button>
        </div>
      </Modal>

      <Toast text={toast?.text ?? null} danger={toast?.danger} />
    </div>
  )
}
