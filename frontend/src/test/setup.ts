import '@testing-library/jest-dom'

// Ensure localStorage is available (needed when auth store initializes)
if (typeof localStorage === 'undefined' || typeof localStorage.getItem !== 'function') {
  const localStorageImpl = (() => {
    let store: Record<string, string> = {}
    return {
      getItem: (k: string) => store[k] ?? null,
      setItem: (k: string, v: string) => { store[k] = v },
      removeItem: (k: string) => { delete store[k] },
      clear: () => { store = {} },
      key: (i: number) => Object.keys(store)[i] ?? null,
      get length() { return Object.keys(store).length },
    }
  })()
  Object.defineProperty(globalThis, 'localStorage', {
    value: localStorageImpl,
    writable: true,
    configurable: true,
  })
}
