import { QueryClient } from '@tanstack/vue-query'
import { describe, expect, it } from 'vitest'
import { invalidateTagged, tagged } from '../query'

const key = (id: string, tags?: string[]) => [{ _id: id, ...(tags ? { tags } : {}) }]

describe('tagged queries', () => {
  it('recognizes the tags of generated keys only', () => {
    expect(tagged(key('listSites', ['configuration']), ['configuration'])).toBe(true)
    expect(tagged(key('status', ['gateway']), ['configuration'])).toBe(false)
    expect(tagged(key('openapi'), ['configuration'])).toBe(false)
    expect(tagged(['plain'], ['configuration'])).toBe(false)
  })

  it('refreshes only the queries of the tags named', async () => {
    const client = new QueryClient()
    client.setQueryData(key('listSites', ['configuration']), [])
    client.setQueryData(key('listApprovalRequests', ['approvals']), [])
    client.setQueryData(key('status', ['gateway']), {})
    client.setQueryData(key('ownSessions', ['identity']), [])
    client.setQueryData(['plain'], [])

    await invalidateTagged(client, ['configuration', 'approvals'])

    const refreshed = client
      .getQueryCache()
      .getAll()
      .filter((query) => query.state.isInvalidated)
      .map((query) => (query.queryKey[0] as { _id: string })._id)
    expect(refreshed.sort()).toEqual(['listApprovalRequests', 'listSites'])
  })
})
