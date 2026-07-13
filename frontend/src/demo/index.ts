// Demo-mode detection.
//
// Demo mode swaps the real `/api` client for an in-memory one (see ./client.ts)
// so the app runs with no backend. It turns on either at build time or at
// runtime:
//
//   • Build time — set `VITE_DEMO=1` (e.g. `VITE_DEMO=1 npm run build`) to ship
//     a static, backend-free demo bundle.
//   • Runtime — append `?demo` to the URL of any normal build to flip it on
//     (persisted in localStorage so it survives navigation); `?demo=0` flips it
//     back off. Handy for toggling a deployed instance into demo for a pitch.

const STORAGE_KEY = 'pulp:demo'

export function isDemoMode(): boolean {
  if (import.meta.env.VITE_DEMO === '1' || import.meta.env.VITE_DEMO === 'true') {
    return true
  }
  if (typeof window === 'undefined') return false
  try {
    const params = new URLSearchParams(window.location.search)
    if (params.has('demo')) {
      const on = params.get('demo') !== '0' && params.get('demo') !== 'false'
      window.localStorage.setItem(STORAGE_KEY, on ? '1' : '0')
      return on
    }
    return window.localStorage.getItem(STORAGE_KEY) === '1'
  } catch {
    return false
  }
}
