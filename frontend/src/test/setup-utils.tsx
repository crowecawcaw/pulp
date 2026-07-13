import { render, type RenderOptions } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactElement } from 'react'

// A QueryClient tuned for deterministic tests: no retries, and crucially no
// polling (`refetchInterval: false`) so the 5s production poll doesn't leave
// timers running and hang the test runner. Caches never expire/go stale so a
// rendered query keeps its data for the test's lifetime.
export function createTestQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        refetchInterval: false,
        refetchOnWindowFocus: false,
        staleTime: Infinity,
        gcTime: Infinity,
      },
      mutations: { retry: false },
    },
  })
}

// Drop-in replacement for RTL's `render` that wraps the tree in a fresh,
// test-tuned QueryClientProvider. Compose with MemoryRouter/ToastProvider as
// before — those still go inside the `ui` argument.
export function renderWithQuery(ui: ReactElement, options?: RenderOptions) {
  const client = createTestQueryClient()
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
    options,
  )
}
