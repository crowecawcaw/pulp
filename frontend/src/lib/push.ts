// Web Push helpers: capability detection, PWA-install detection, and the
// subscribe flow that registers this device with the backend.
import { api } from '@/api/client'

/** Whether this browser can do Web Push at all. */
export function pushSupported(): boolean {
  return (
    typeof navigator !== 'undefined' &&
    'serviceWorker' in navigator &&
    typeof window !== 'undefined' &&
    'PushManager' in window &&
    'Notification' in window
  )
}

/** True when running as an installed PWA (standalone display mode). On iOS,
 *  Web Push only works once the app is added to the Home Screen. */
export function isStandalone(): boolean {
  if (typeof window === 'undefined') return false
  const iosStandalone =
    (window.navigator as unknown as { standalone?: boolean }).standalone === true
  return window.matchMedia?.('(display-mode: standalone)').matches || iosStandalone
}

/** iOS/iPadOS detection (iPadOS can report as desktop Safari with touch). */
export function isIos(): boolean {
  if (typeof navigator === 'undefined') return false
  const ua = navigator.userAgent
  return (
    /iphone|ipad|ipod/i.test(ua) ||
    (/macintosh/i.test(ua) && typeof document !== 'undefined' && 'ontouchend' in document)
  )
}

function urlBase64ToUint8Array(base64: string): Uint8Array<ArrayBuffer> {
  const padding = '='.repeat((4 - (base64.length % 4)) % 4)
  const b64 = (base64 + padding).replace(/-/g, '+').replace(/_/g, '/')
  const raw = atob(b64)
  // Back it with a concrete ArrayBuffer so the result satisfies the DOM's
  // `applicationServerKey: BufferSource` (which requires an ArrayBuffer-backed
  // view, not the generic ArrayBufferLike one `new Uint8Array(len)` infers).
  const arr = new Uint8Array(new ArrayBuffer(raw.length))
  for (let i = 0; i < raw.length; i++) arr[i] = raw.charCodeAt(i)
  return arr
}

/** The webpush `config` blob stored on a notification: the browser push
 *  subscription's endpoint and encryption keys. */
export interface WebPushConfig {
  endpoint: string
  p256dh: string
  auth: string
}

/** Request notification permission and subscribe THIS device to web push,
 *  returning the subscription's endpoint + keys. The caller persists this as a
 *  `webpush` notification (`POST /api/notifications`). Throws on denial or any
 *  failure. Must be called from a user gesture so the permission prompt is
 *  allowed. */
export async function enablePush(): Promise<WebPushConfig> {
  if (!pushSupported()) {
    throw new Error('Push notifications are not supported in this browser.')
  }

  const permission = await Notification.requestPermission()
  if (permission !== 'granted') {
    throw new Error('Notification permission was not granted.')
  }

  const reg = await navigator.serviceWorker.ready
  const { key } = await api.push.vapidPublicKey()

  let sub = await reg.pushManager.getSubscription()
  if (!sub) {
    sub = await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(key),
    })
  }

  const json = sub.toJSON()
  if (!json.endpoint || !json.keys?.p256dh || !json.keys?.auth) {
    throw new Error('Subscription is missing its encryption keys.')
  }
  return { endpoint: json.endpoint, p256dh: json.keys.p256dh, auth: json.keys.auth }
}

/** The push endpoint this device is already subscribed to, if any. Lets the UI
 *  detect that the current device already has a matching `webpush` notification
 *  (so it can show "enabled" + a remove control instead of offering to add it
 *  again). Returns null when unsupported or not yet subscribed. */
export async function currentPushEndpoint(): Promise<string | null> {
  if (!pushSupported()) return null
  try {
    const reg = await navigator.serviceWorker.ready
    const sub = await reg.pushManager.getSubscription()
    return sub?.endpoint ?? null
  } catch {
    return null
  }
}

/** A sensible default label for a web-push notification created on this device,
 *  e.g. "iPhone — Safari". Best-effort UA sniffing; the user never edits it. */
export function deviceLabel(): string {
  if (typeof navigator === 'undefined') return 'this device'
  const ua = navigator.userAgent
  const device =
    /iphone/i.test(ua) ? 'iPhone' :
    /ipad/i.test(ua) ? 'iPad' :
    /android/i.test(ua) ? 'Android' :
    /macintosh|mac os x/i.test(ua) ? 'Mac' :
    /windows/i.test(ua) ? 'Windows' :
    /linux/i.test(ua) ? 'Linux' :
    'this device'
  const browser =
    /edg\//i.test(ua) ? 'Edge' :
    /chrome|crios/i.test(ua) ? 'Chrome' :
    /firefox|fxios/i.test(ua) ? 'Firefox' :
    /safari/i.test(ua) ? 'Safari' :
    'browser'
  return `${device} — ${browser}`
}
