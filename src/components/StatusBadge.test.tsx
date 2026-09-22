/**
 * Бейдж статуса.
 *
 * Статусы настраиваются в каждой группе своими, поэтому рисовать их по
 * зашитому в клиент списку нельзя — свой статус показался бы чужим именем и
 * чужим цветом. Раньше так и было, а сам бейдж существовал в четырёх копиях.
 */
import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { StatusBadge, StatusDot } from '@/components/StatusBadge'

const CUSTOM = { name: 'На поверке', color: '#B45309', bg: '#FEF3C7' }

describe('StatusBadge', () => {
  it('показывает название и цвета из настроек группы', () => {
    render(<StatusBadge status={CUSTOM} />)
    const badge = screen.getByText('На поверке')
    // Свой статус должен выглядеть своим, а не подставленным из списка.
    expect(badge.style.backgroundColor).toBe('rgb(254, 243, 199)')
    expect(badge.style.color).toBe('rgb(180, 83, 9)')
  })

  it('ставит прочерк, когда статуса нет', () => {
    render(<StatusBadge status={null} />)
    expect(screen.getByText('—')).toBeTruthy()
  })

  it('не падает на неизвестном статусе', () => {
    // Раньше неизвестный статус подменялся первым из зашитого списка —
    // человек видел чужое название и не понимал, почему.
    render(<StatusBadge status={{ name: 'Выдумка', color: '#000', bg: '#fff' }} />)
    expect(screen.getByText('Выдумка')).toBeTruthy()
  })

  it('точка берёт цвет оттуда же', () => {
    const { container } = render(<StatusDot status={CUSTOM} />)
    expect(screen.getByText('На поверке')).toBeTruthy()
    const dot = container.querySelector('span span')
    expect((dot as HTMLElement).style.background).toBe('rgb(180, 83, 9)')
  })

  it('точка без статуса тоже показывает прочерк', () => {
    render(<StatusDot status={undefined} />)
    expect(screen.getByText('—')).toBeTruthy()
  })
})
