/**
 * Кого страница входа пускает дальше, а кому показывает форму.
 *
 * Проверяется не оформление, а одно решение: показать поля или увести.
 * Ошибка здесь стоила того, что приложение на телефоне при каждом запуске
 * просило войти заново — сессия была жива, но экран открывался тот же.
 * Увидеть это можно было только перезапуском приложения на устройстве,
 * поэтому решение вынесено в тест.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'

type QueryState = { data?: unknown; isLoading: boolean }

const meState: QueryState = { data: undefined, isLoading: false }
const wsState: QueryState = { data: [], isLoading: false }

vi.mock('@/providers/trpc', () => ({
  trpc: {
    auth: { me: { useQuery: () => meState } },
    meta: { workspaces: { useQuery: () => wsState } },
  },
}))

// Куда увели — единственное, что важно; рисовать целевой экран незачем.
vi.mock('react-router', () => ({
  Navigate: ({ to }: { to: string }) => <div data-testid="redirect">{to}</div>,
}))

vi.mock('@/components/auth/AuthForm', () => ({
  default: () => <div data-testid="form">форма входа</div>,
}))
vi.mock('@/components/auth/BrandPanel', () => ({ default: () => null }))
vi.mock('@/components/auth/MeshCanvas', () => ({ default: () => null }))
vi.mock('sonner', () => ({ Toaster: () => null }))

const { default: Auth } = await import('@/pages/Auth')

beforeEach(() => {
  meState.data = undefined
  meState.isLoading = false
  wsState.data = []
  wsState.isLoading = false
})

describe('страница входа', () => {
  it('без сессии показывает форму', () => {
    render(<Auth />)
    expect(screen.getByTestId('form')).toBeTruthy()
    expect(screen.queryByTestId('redirect')).toBeNull()
  })

  it('с живой сессией и организацией ведёт в каталог', () => {
    meState.data = { id: 1, fullName: 'Иванов' }
    wsState.data = [{ id: 1, name: 'Бригада' }]
    render(<Auth />)
    expect(screen.getByTestId('redirect').textContent).toBe('/')
    expect(screen.queryByTestId('form')).toBeNull()
  })

  it('с сессией, но без организации ведёт на выбор организации', () => {
    meState.data = { id: 1, fullName: 'Иванов' }
    wsState.data = []
    render(<Auth />)
    expect(screen.getByTestId('redirect').textContent).toBe('/start')
  })

  it('пока проверяет сессию, форму не показывает', () => {
    // Показать поля и через мгновение их убрать — выглядит как сбой.
    meState.isLoading = true
    render(<Auth />)
    expect(screen.queryByTestId('form')).toBeNull()
    expect(screen.queryByTestId('redirect')).toBeNull()
  })

  it('не решает, куда вести, пока не знает про организации', () => {
    meState.data = { id: 1, fullName: 'Иванов' }
    wsState.isLoading = true
    wsState.data = undefined
    render(<Auth />)
    // Иначе человека с организацией на миг унесло бы на экран её создания.
    expect(screen.queryByTestId('redirect')).toBeNull()
  })
})
