/**
 * Поле «выбрать или завести новое» из формы создания инструмента.
 *
 * Логика неочевидная: список подменяет своё значение, пока открыт ввод, а
 * при отказе сервера набранный текст должен остаться на экране. Проверялось
 * это до сих пор только руками.
 */
import { describe, it, expect, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { SelectOrCreate } from '@/pages/CreateTool'

const OPTIONS = [
  { id: 1, name: 'Электроинструмент' },
  { id: 2, name: 'Расходники' },
]

function setup(props: Partial<React.ComponentProps<typeof SelectOrCreate>> = {}) {
  const onChange = vi.fn()
  const onCreate = vi.fn().mockResolvedValue(undefined)
  render(
    <SelectOrCreate
      value={null}
      onChange={onChange}
      options={OPTIONS}
      emptyLabel="Выберите категорию"
      addLabel="+ Новая категория"
      placeholder="Название категории"
      canAdd
      onCreate={onCreate}
      {...props}
    />,
  )
  return { onChange, onCreate }
}

describe('SelectOrCreate', () => {
  it('не показывает пункт создания тем, кому нельзя', () => {
    setup({ canAdd: false })
    expect(screen.queryByRole('option', { name: '+ Новая категория' })).toBeNull()
    // Сам список при этом остаётся рабочим.
    expect(screen.getByRole('option', { name: 'Электроинструмент' })).toBeTruthy()
  })

  it('открывает поле ввода по выбору пункта создания', async () => {
    const user = userEvent.setup()
    const { onChange } = setup()
    await user.selectOptions(screen.getByRole('combobox'), '__new__')

    expect(screen.getByPlaceholderText('Название категории')).toBeTruthy()
    // Выбор «создать» — не выбор значения: наружу ничего уходить не должно.
    expect(onChange).not.toHaveBeenCalled()
  })

  it('заводит запись и закрывает ввод', async () => {
    const user = userEvent.setup()
    const { onCreate } = setup()
    await user.selectOptions(screen.getByRole('combobox'), '__new__')
    await user.type(screen.getByPlaceholderText('Название категории'), 'Лестницы')
    await user.click(screen.getByRole('button', { name: 'Добавить' }))

    expect(onCreate).toHaveBeenCalledWith('Лестницы')
    await waitFor(() => {
      expect(screen.queryByPlaceholderText('Название категории')).toBeNull()
    })
  })

  it('оставляет набранное на экране, если сервер отказал', async () => {
    const user = userEvent.setup()
    const onCreate = vi.fn().mockRejectedValue(new Error('сеть отвалилась'))
    render(
      <SelectOrCreate
        value={null}
        onChange={vi.fn()}
        options={OPTIONS}
        emptyLabel="Выберите категорию"
        addLabel="+ Новая категория"
        placeholder="Название категории"
        canAdd
        onCreate={onCreate}
      />,
    )
    await user.selectOptions(screen.getByRole('combobox'), '__new__')
    const input = screen.getByPlaceholderText('Название категории')
    await user.type(input, 'Лестницы')
    await user.click(screen.getByRole('button', { name: 'Добавить' }))

    // Терять введённое из-за случайной ошибки сети нельзя.
    await waitFor(() => {
      expect((screen.getByPlaceholderText('Название категории') as HTMLInputElement).value).toBe(
        'Лестницы',
      )
    })
  })

  it('не отправляет слишком короткое название', async () => {
    const user = userEvent.setup()
    const { onCreate } = setup()
    await user.selectOptions(screen.getByRole('combobox'), '__new__')
    await user.type(screen.getByPlaceholderText('Название категории'), 'Л')

    const button = screen.getByRole('button', { name: 'Добавить' }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    await user.click(button)
    expect(onCreate).not.toHaveBeenCalled()
  })

  it('отменяет ввод по Escape', async () => {
    const user = userEvent.setup()
    setup()
    await user.selectOptions(screen.getByRole('combobox'), '__new__')
    await user.type(screen.getByPlaceholderText('Название категории'), 'Лестницы{Escape}')
    expect(screen.queryByPlaceholderText('Название категории')).toBeNull()
  })
})
