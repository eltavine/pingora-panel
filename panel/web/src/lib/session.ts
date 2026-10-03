import { computed } from 'vue'
import { useQuery } from '@tanstack/vue-query'
import { logout, type CurrentSession } from '@/api/generated'
import { client } from '@/api/generated/client.gen'
import { sessionOptions, sessionQueryKey } from '@/api/generated/@tanstack/vue-query.gen'
import { toApiFailure } from './api'
import { queryClient } from './query'

/**
 * The browser's login session. Its cookie is `HttpOnly`, so the console only
 * keeps what the API returns with it: the account, its permissions and the
 * CSRF token every unsafe request must carry.
 */
let csrfToken: string | undefined

const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS'])

/** Answers of these paths are the sign-in pages' own business. */
const SIGN_IN_PATHS = ['/api/v1/session', '/api/v1/setup']

/** Keeps the CSRF token of `session` for the requests that follow. */
export function adoptSession(session: CurrentSession | null | undefined) {
  csrfToken = session?.csrf_token ?? undefined
  if (session) {
    queryClient.setQueryData(sessionQueryKey(), session)
  } else {
    queryClient.removeQueries({ queryKey: sessionQueryKey() })
  }
}

/** The current session, or null when the browser is not logged in. */
export async function loadSession(): Promise<CurrentSession | null> {
  try {
    const session = await queryClient.fetchQuery({ ...sessionOptions(), staleTime: 30_000 })
    csrfToken = session.csrf_token ?? undefined
    return session
  } catch (error) {
    const failure = toApiFailure(error)
    if (failure.kind === 'problem' && failure.problem.status === 401) {
      return null
    }
    throw error
  }
}

/** Ends the session; the server clears its cookie. */
export async function signOut() {
  try {
    await logout()
  } finally {
    csrfToken = undefined
    queryClient.clear()
  }
}

/**
 * Sends the CSRF token with unsafe requests and reports a session that
 * ended while the console was open.
 */
export function installSession(onSignedOut: () => void) {
  client.interceptors.request.use((request) => {
    if (csrfToken && !SAFE_METHODS.has(request.method)) {
      request.headers.set('x-csrf-token', csrfToken)
    }
    return request
  })
  client.interceptors.response.use((response, request) => {
    const path = new URL(request.url, window.location.origin).pathname
    if (response.status === 401 && !SIGN_IN_PATHS.includes(path)) {
      csrfToken = undefined
      queryClient.removeQueries({ queryKey: sessionQueryKey() })
      onSignedOut()
    }
    return response
  })
}

/** The logged-in account and what it may do. */
export function useSession() {
  const query = useQuery({ ...sessionOptions(), staleTime: 30_000, retry: false })
  const permissions = computed(() => new Set(query.data.value?.permissions ?? []))
  return {
    session: query.data,
    can: (permission: string) => permissions.value.has(permission),
  }
}
