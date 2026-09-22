/**
 * Печать QR-этикеток на предметы.
 *
 * Раньше кнопка «Печать QR» показывала надпись «QR-коды отправлены на
 * печать» и не делала ничего. Это хуже отсутствующей кнопки: человек
 * уходил к принтеру за листом, которого нет.
 *
 * Печатаем через отдельное окно, а не через `@media print` на текущей
 * странице: на телефоне печать идёт через системный диалог, и отдавать ему
 * весь интерфейс каталога бессмысленно — на лист нужны только наклейки.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { createElement } from 'react'
import { QRCodeSVG } from 'qrcode.react'

export type QrLabel = {
  /** Что зашито в код: QR предмета, иначе внутренний номер. */
  code: string
  /** Внутренний номер — крупно под кодом. */
  vn: string
  /** Название — мелко, обрезается по ширине наклейки. */
  name: string
}

/** Экранирование для подстановки в разметку листа. */
function escapeHtml(raw: string): string {
  return raw
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

/**
 * Собирает лист наклеек.
 *
 * Вынесено отдельно от печати, чтобы это можно было проверить тестом:
 * открыть окно печати в тесте нельзя, а вот убедиться, что в лист попали
 * все предметы и ничьё название не сломало разметку, — можно.
 */
export function buildQrSheet(labels: QrLabel[], title = 'QR-этикетки'): string {
  const cells = labels
    .map((label) => {
      const svg = renderToStaticMarkup(
        createElement(QRCodeSVG, { value: label.code, size: 128, level: 'M' }),
      )
      return `<figure class="label">
        ${svg}
        <figcaption><b>${escapeHtml(label.vn)}</b><span>${escapeHtml(label.name)}</span></figcaption>
      </figure>`
    })
    .join('')

  return `<!doctype html>
<html lang="ru"><head><meta charset="utf-8"><title>${escapeHtml(title)}</title>
<style>
  @page { margin: 10mm; }
  body { margin: 0; font: 12px/1.3 system-ui, sans-serif; color: #111; }
  .sheet { display: flex; flex-wrap: wrap; gap: 6mm; }
  .label {
    margin: 0; width: 45mm; padding: 3mm;
    border: 1px dashed #bbb; border-radius: 2mm;
    display: flex; flex-direction: column; align-items: center; gap: 2mm;
    /* Наклейку нельзя разрывать между страницами: половина кода
       не сканируется. */
    break-inside: avoid; page-break-inside: avoid;
  }
  .label svg { width: 30mm; height: 30mm; }
  figcaption { text-align: center; width: 100%; }
  figcaption b { display: block; font-size: 13px; font-variant-numeric: tabular-nums; }
  figcaption span {
    display: block; font-size: 10px; color: #555;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
</style></head>
<body><div class="sheet">${cells}</div></body></html>`
}

/**
 * Открывает системный диалог печати с листом наклеек.
 *
 * Возвращает описание неудачи или `null`, если лист ушёл в печать. Чаще
 * всего мешает блокировщик всплывающих окон — и сказать об этом надо
 * прямо, иначе нажатие выглядит как «ничего не произошло».
 */
export function printQrLabels(labels: QrLabel[]): string | null {
  if (labels.length === 0) return 'Нечего печатать: выберите предметы'
  const win = window.open('', '_blank')
  if (!win) return 'Браузер заблокировал окно печати. Разрешите всплывающие окна'
  win.document.write(buildQrSheet(labels))
  win.document.close()
  win.focus()
  // Даём разметке отрисоваться: без этого часть браузеров печатает
  // пустой лист, потому что SVG ещё не разложены.
  win.setTimeout(() => win.print(), 250)
  return null
}
