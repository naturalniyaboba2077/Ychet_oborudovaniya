/**
 * Лист QR-этикеток.
 *
 * Сам диалог печати в тесте не откроешь, а вот лист собрать и проверить —
 * можно. Раньше кнопка печати не делала ничего, поэтому важно убедиться,
 * что теперь в лист попадают все выбранные предметы и что чужое название
 * не может сломать разметку.
 */
import { describe, it, expect } from 'vitest'
import { buildQrSheet } from '@/lib/print-qr'

describe('buildQrSheet', () => {
  it('делает по наклейке на каждый предмет', () => {
    const html = buildQrSheet([
      { code: 'MK-1', vn: 'ВН-0001', name: 'Перфоратор' },
      { code: 'MK-2', vn: 'ВН-0002', name: 'Шуруповёрт' },
    ])
    expect(html.match(/class="label"/g)?.length).toBe(2)
    expect(html).toContain('ВН-0001')
    expect(html).toContain('Шуруповёрт')
  })

  it('рисует сам код, а не подпись к нему', () => {
    const html = buildQrSheet([{ code: 'MK-1', vn: 'ВН-0001', name: 'Перфоратор' }])
    // Без <svg> лист печатается с подписями и пустыми местами вместо кодов.
    expect(html).toContain('<svg')
  })

  it('не даёт названию сломать разметку', () => {
    const html = buildQrSheet([
      { code: 'MK-1', vn: 'ВН-0001', name: '<script>alert(1)</script>' },
    ])
    expect(html).not.toContain('<script>alert(1)</script>')
    expect(html).toContain('&lt;script&gt;')
  })

  it('пустой список даёт пустой лист, а не поломку', () => {
    const html = buildQrSheet([])
    expect(html).toContain('<div class="sheet"></div>')
  })

  it('запрещает разрывать наклейку между страницами', () => {
    // Половина кода на одной странице, половина на другой — не сканируется.
    expect(buildQrSheet([{ code: 'MK-1', vn: 'ВН-1', name: 'Молоток' }])).toContain(
      'page-break-inside: avoid',
    )
  })
})
