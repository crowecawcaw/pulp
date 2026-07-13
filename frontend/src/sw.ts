/// <reference lib="webworker" />
//
// Custom service worker (vite-plugin-pwa `injectManifest`). It does two jobs:
//   1. Precaches the built app shell and serves the SPA navigation fallback.
//   2. Receives Web Push messages and shows notifications.
//
// `self` is the ServiceWorkerGlobalScope; we alias it through a cast rather than
// re-declaring the global (which the WebWorker lib already provides).
import { precacheAndRoute, createHandlerBoundToURL } from 'workbox-precaching'
import { NavigationRoute, registerRoute } from 'workbox-routing'

const sw = self as unknown as ServiceWorkerGlobalScope

// `__WB_MANIFEST` is replaced at build time with the precache manifest.
type PrecacheEntry = { url: string; revision: string | null }
precacheAndRoute(
  (self as unknown as { __WB_MANIFEST: PrecacheEntry[] }).__WB_MANIFEST,
)

// SPA fallback: serve index.html for navigations, except API requests, which
// must always hit the network (REST + the SSE stream).
registerRoute(
  new NavigationRoute(createHandlerBoundToURL('index.html'), {
    denylist: [/^\/api\//],
  }),
)

// Apply updates immediately so a refresh always lands on the latest build.
sw.addEventListener('install', () => {
  sw.skipWaiting()
})
sw.addEventListener('activate', (event) => {
  event.waitUntil(sw.clients.claim())
})

interface PushPayload {
  title?: string
  body?: string
  url?: string
}

sw.addEventListener('push', (event: PushEvent) => {
  let data: PushPayload
  try {
    data = event.data?.json() ?? {}
  } catch {
    data = { body: event.data?.text() }
  }
  const title = data.title ?? 'Pulp'
  event.waitUntil(
    sw.registration.showNotification(title, {
      body: data.body ?? '',
      icon: '/pwa-192x192.png',
      badge: '/pwa-192x192.png',
      data: { url: data.url ?? '/' },
    }),
  )
})

sw.addEventListener('notificationclick', (event: NotificationEvent) => {
  event.notification.close()
  const url = (event.notification.data?.url as string | undefined) ?? '/'
  event.waitUntil(
    (async () => {
      const windows = await sw.clients.matchAll({
        type: 'window',
        includeUncontrolled: true,
      })
      // Reuse an open tab: focus it AND navigate it to the mention. The old
      // code only focused, so clicking a notification while the app was open
      // just surfaced whatever page it was already on. `navigate` is best-effort
      // (it can reject for an uncontrolled/cross-origin tab) — focusing still
      // happens regardless.
      for (const client of windows) {
        const wc = client as WindowClient
        await wc.focus()
        if ('navigate' in wc) {
          try {
            await wc.navigate(url)
          } catch {
            // keep the focused tab as-is if navigation isn't permitted
          }
        }
        return
      }
      // No tab open — open the deep link in a fresh window.
      await sw.clients.openWindow(url)
    })(),
  )
})
