export type InviteRole = 'member' | 'admin' | 'viewer'

export const INVITE_ROLES: Array<{ value: InviteRole; label: string; hint: string }> = [
  { value: 'member', label: 'Участник', hint: 'берёт и возвращает оборудование' },
  { value: 'admin', label: 'Администратор', hint: 'ведёт каталог, заявки и инвентаризацию' },
  { value: 'viewer', label: 'Наблюдатель', hint: 'только смотрит каталог' },
]

export const INVITE_TTL_HOURS = 168

/**
 * Роли, которые человек может выдать приглашением. Администраторов зовёт
 * только создатель пространства — сервер откажет остальным
 * (`ws_create_invite`), поэтому и кнопку им не показываем.
 */
export function inviteRolesFor(isCreator: boolean) {
  return isCreator ? INVITE_ROLES : INVITE_ROLES.filter((r) => r.value !== 'admin')
}

/** Подпись роли из приглашения: «Участник», «Администратор»… */
export function inviteRoleLabel(role?: string | null): string {
  return INVITE_ROLES.find((r) => r.value === role)?.label ?? 'Участник'
}

/**
 * Достаёт код приглашения из того, что человек вставил или отсканировал:
 * ссылки `…/join?token=…`, QR с JSON `{"t":"join","token":…}` или самого
 * кода. Ссылку пересылают в мессенджере, код диктуют, QR показывают с
 * экрана — принимать надо всё. Пустая строка — кода не нашлось.
 */
export function parseInviteToken(raw: string): string {
  const value = raw.trim()
  if (!value) return ''
  try {
    const parsed = JSON.parse(value) as { t?: string; token?: string }
    if (parsed && typeof parsed === 'object') {
      return parsed.t === 'join' && typeof parsed.token === 'string' ? parsed.token.trim() : ''
    }
  } catch {
    /* не JSON */
  }
  const m = value.match(/[?&]token=([^&#\s]+)/)
  if (m) return decodeURIComponent(m[1])
  // Ссылка без кода — не код.
  if (/^[a-z]+:\/\//i.test(value) || /\s/.test(value)) return ''
  return value
}

type InviteLike = {
  token: string
  usable?: boolean
  expired?: boolean
  revoked?: boolean
  usedCount?: number
  maxUses?: number
}

/**
 * Первое приглашение, которым ещё можно воспользоваться. Показывать
 * просроченный или исчерпанный QR нельзя: он отвалится при сканировании.
 */
export function firstUsableInvite<T extends InviteLike>(list?: T[] | null): T | undefined {
  return list?.find((invite) => {
    if (typeof invite.usable === 'boolean') return invite.usable
    if (invite.revoked || invite.expired) return false
    if (typeof invite.usedCount === 'number' && typeof invite.maxUses === 'number') {
      return invite.usedCount < invite.maxUses
    }
    return true
  })
}

export function inviteExpiryLabel(expiresAt?: string | null): string | null {
  if (!expiresAt) return null
  const date = new Date(expiresAt)
  if (Number.isNaN(date.getTime())) return null
  return date.toLocaleString('ru-RU', { dateStyle: 'short', timeStyle: 'short' })
}
