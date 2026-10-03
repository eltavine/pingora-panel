import { afterEach, describe, expect, it, vi } from 'vitest'
import { listAccounts, logout, session } from '@/api/generated'
import { client } from '@/api/generated/client.gen'
import { adoptSession, installSession } from '../session'

const requests: Request[] = []
let status = 200

client.setConfig({
  baseUrl: 'http://panel.test',
  fetch: async (input: RequestInfo | URL) => {
    requests.push(input as Request)
    return new Response(status === 204 ? null : '{}', {
      status,
      headers: { 'content-type': 'application/json' },
    })
  },
})
const signedOut = vi.fn<() => void>()
installSession(signedOut)

afterEach(() => {
  requests.length = 0
  status = 200
  signedOut.mockReset()
})

const current = {
  account: { id: 'a', username: 'root', roles: [], disabled: false, locked: false },
  permissions: ['identity.read'],
  credential: 'cookie',
  csrf_token: 'the-csrf-token',
} as never

describe('the session', () => {
  it('sends its CSRF token with unsafe requests only', async () => {
    adoptSession(current)
    status = 204
    await logout()
    status = 200
    await listAccounts()
    expect(requests[0]!.method).toBe('DELETE')
    expect(requests[0]!.headers.get('x-csrf-token')).toBe('the-csrf-token')
    expect(requests[1]!.headers.get('x-csrf-token')).toBeNull()
  })

  it('reports a session that ended, except to the sign-in pages', async () => {
    status = 401
    await listAccounts()
    expect(signedOut).toHaveBeenCalledTimes(1)
    await session()
    expect(signedOut).toHaveBeenCalledTimes(1)
  })

  it('forgets the token when the session goes away', async () => {
    adoptSession(null)
    status = 204
    await logout()
    expect(requests[0]!.headers.get('x-csrf-token')).toBeNull()
  })
})
