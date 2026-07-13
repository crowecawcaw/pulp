import { QueryClient } from '@tanstack/react-query'

// Single app-wide QueryClient. The global query defaults encode the user's
// spec: every query polls on a 5s interval, and on mount shows cached data
// immediately while refetching fresh data in the background (staleTime: 0 +
// refetchOnMount). `retry: 1` keeps transient blips from surfacing as errors.
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        refetchInterval: 5000,
        staleTime: 0,
        refetchOnMount: true,
        refetchOnWindowFocus: true,
        retry: 1,
      },
    },
  })
}

export const queryClient = createQueryClient()
