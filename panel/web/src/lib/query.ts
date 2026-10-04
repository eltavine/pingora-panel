import { QueryClient, type QueryKey } from '@tanstack/vue-query'

/** The console's one query cache, shared by components and navigation guards. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 5_000, retry: 1, refetchOnWindowFocus: true },
    // Mutations carry idempotency keys; retries are explicit user actions.
    mutations: { retry: false },
  },
})

/** Whether a generated query key belongs to an operation with one of `tags`. */
export function tagged(key: QueryKey, tags: readonly string[]): boolean {
  const [head] = key
  if (typeof head !== 'object' || head === null || !('tags' in head)) {
    return false
  }
  return Array.isArray(head.tags) && head.tags.some((tag) => tags.includes(tag))
}

/** Refreshes the cached answers of operations with one of `tags`. */
export function invalidateTagged(client: QueryClient, tags: readonly string[]): Promise<void> {
  return client.invalidateQueries({ predicate: (query) => tagged(query.queryKey, tags) })
}
