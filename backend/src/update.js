// Переход открытого приложения на новую сборку интерфейса.
//
// Сервер вставляет этот скрипт в index.html вместе с меткой сборки
// (window.__MK_BUILD) и сообщает о новой сборке по SSE, см. update.rs.
//
// Когда перезагружать:
//  - приложение в фоне или человек только что к нему вернулся — сразу,
//    он этого не заметит;
//  - человек работает на экране — показываем плашку с кнопкой: молча
//    перезагрузить значит стереть недозаполненную форму. Уйдёт в фон —
//    обновимся сами.
(function () {
  'use strict'
  var loaded = window.__MK_BUILD
  if (!loaded || !window.fetch) return

  var GUARD = 'mk-reloaded-for'
  var fresh = null
  var banner = null

  // Защита от вечной перезагрузки: если ради этой сборки уже перезагружались,
  // а страница всё равно старая (прокси, кэш), второй раз молча не пробуем.
  function reloadOnce(build) {
    try {
      if (sessionStorage.getItem(GUARD) === build) return false
      sessionStorage.setItem(GUARD, build)
    } catch (e) {}
    location.reload()
    return true
  }

  function onBuild(build, resumed) {
    if (!build || build === loaded) return
    fresh = build
    if (document.visibilityState === 'hidden' || resumed) {
      if (reloadOnce(build)) return
    }
    showBanner()
  }

  function check(resumed) {
    fetch('/api/app/version', { cache: 'no-store', credentials: 'same-origin' })
      .then(function (r) { return r.ok ? r.json() : null })
      .then(function (v) { if (v) onBuild(v.build, resumed) })
      .catch(function () {})
  }

  function showBanner() {
    if (banner || !document.body) return
    banner = document.createElement('div')
    banner.setAttribute('role', 'status')
    banner.style.cssText =
      'position:fixed;left:50%;transform:translateX(-50%);' +
      'top:calc(env(safe-area-inset-top, 0px) + 12px);z-index:2147483647;' +
      'display:flex;align-items:center;gap:12px;max-width:calc(100% - 32px);' +
      'padding:10px 10px 10px 16px;border-radius:14px;background:#5E629B;color:#fff;' +
      'font:500 14px/1.3 "Exo 2",system-ui,sans-serif;box-shadow:0 8px 24px rgba(0,0,0,.25)'
    var text = document.createElement('span')
    text.textContent = 'Вышла новая версия'
    var button = document.createElement('button')
    button.type = 'button'
    button.textContent = 'Обновить'
    button.style.cssText =
      'border:0;border-radius:10px;padding:8px 14px;background:#fff;color:#5E629B;' +
      'font:inherit;font-weight:600;cursor:pointer'
    button.onclick = function () {
      // Явное нажатие — перезагружаем всегда, защита от петли тут не нужна.
      location.reload()
    }
    banner.appendChild(text)
    banner.appendChild(button)
    document.body.appendChild(banner)
  }

  document.addEventListener('visibilitychange', function () {
    if (document.visibilityState === 'hidden') {
      if (fresh) reloadOnce(fresh)
    } else {
      // Телефон рвёт соединение у спящего приложения, поэтому при
      // возвращении спрашиваем сами, не дожидаясь переподключения.
      check(true)
    }
  })
  window.addEventListener('pageshow', function (e) { if (e.persisted) check(true) })
  window.addEventListener('online', function () { check(false) })

  if (window.EventSource) {
    // EventSource сам переподключается после обрыва и перезапуска сервера.
    var source = new EventSource('/api/app/events')
    source.addEventListener('build', function (e) { onBuild(e.data, false) })
  } else {
    setInterval(function () { check(false) }, 60000)
  }
  check(false)
})()
