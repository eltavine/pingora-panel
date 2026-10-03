import { QueryClient } from '@tanstack/vue-query'

/** The console's one query cache, shared by components and navigation guards. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 5_000, retry: 1, refetchOnWindowFocus: true },
    // Mutations carry idempotency keys; retries are explicit user actions.
    mutations: { retry: false },
  },
})
