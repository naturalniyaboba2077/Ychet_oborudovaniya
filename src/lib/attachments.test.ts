/**
 * Предел на размер вложения — одно число в двух местах.
 *
 * Разойдутся — клиент пропустит файл, который сервер отвергнет, и человек
 * узнает об этом уже после заполнения всей формы. Поправить одну половину
 * и забыть про вторую очень легко, а заметить это на глаз — нет.
 */
import { describe, it, expect } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { MAX_ATTACHMENT_BYTES } from '@/lib/attachments'

describe('предел вложения', () => {
  it('совпадает с MAX_ATTACHMENT_BYTES в Rust', () => {
    const source = readFileSync(path.resolve(process.cwd(), 'backend/src/api/items.rs'), 'utf8')
    const found = source.match(
      /MAX_ATTACHMENT_BYTES:\s*usize\s*=\s*(\d+)\s*\*\s*(\d+)\s*\*\s*(\d+)/,
    )
    expect(found, 'в items.rs не найден предел вложения').not.toBeNull()
    const [, a, b, c] = found!
    expect(Number(a) * Number(b) * Number(c)).toBe(MAX_ATTACHMENT_BYTES)
  })
})
