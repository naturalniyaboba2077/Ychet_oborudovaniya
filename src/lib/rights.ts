/**
 * Права текущего человека в текущей организации.
 *
 * Раньше интерфейс смотрел в `roleRights` из профиля. Это работало ровно до
 * того, как один аккаунт смог состоять в нескольких организациях: права
 * лежат в профиле одни, а в каждой организации — свои. Каталог показывал
 * кнопку «Списать всё» рядовому работнику, тот нажимал, и сервер отвечал
 * «списание доступно только руководителю». Кнопки, которые нельзя нажать,
 * хуже отсутствующих: человек считает, что приложение сломалось.
 *
 * Это подсказка для интерфейса, а не защита. Решает всё равно сервер —
 * `require_can_in_workspace` в `backend/src/api/mod.rs`.
 */
import { useMemo } from 'react'
import { trpc } from '@/providers/trpc'
import { useStore } from '@/lib/store'

export type RightKey =
  | 'viewItems'
  | 'createItems'
  | 'editItems'
  | 'deleteItems'
  | 'transferItems'
  | 'acceptTransfers'
  | 'writeOff'
  | 'replenish'
  | 'inventory'
  | 'viewHistory'
  | 'viewReports'
  | 'manageUsers'
  | 'manageWorkspaces'
  | 'manageStorages'
  | 'manageSites'
  | 'manageDictionaries'

export type Rights = Partial<Record<RightKey, boolean>>

/**
 * `can('writeOff')` — можно ли показывать кнопку.
 *
 * Пока права не загружены, отвечает «нельзя»: показать кнопку и тут же её
 * отнять — хуже, чем показать на полсекунды позже.
 */
export function useCan(): (key: RightKey) => boolean {
  const { workspace } = useStore()
  const wsQ = trpc.meta.workspaces.useQuery(undefined, { retry: 0 })
  const meQ = trpc.meta.currentUser.useQuery(undefined, { retry: 0 })

  return useMemo(() => {
    const list = (wsQ.data ?? []) as Array<{ id: number; rights?: Rights | null }>
    const here = list.find((w) => w.id === workspace?.id)
    // Запасной вариант — права из профиля. Он нужен только пока сервер
    // старой версии не отдаёт права рядом с пространством; на новом
    // сервере сюда не доходит.
    const fallback = (meQ.data?.roleRights ?? null) as Rights | null
    const rights = here?.rights ?? fallback
    return (key: RightKey) => rights?.[key] === true
  }, [wsQ.data, meQ.data, workspace?.id])
}
