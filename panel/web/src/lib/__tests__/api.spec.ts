import { describe, expect, it } from 'vitest'
import { toApiFailure } from '../api'

describe('toApiFailure', () => {
  it('keeps RFC 9457 problem documents with their stable code', () => {
    const problem = {
      type: 'about:blank',
      title: 'Conflict',
      status: 409,
      detail: 'the active hash changed',
      code: 'CONFLICT',
      retryable: false,
      request_id: 'req-1',
    }

    expect(toApiFailure(problem)).toEqual({ kind: 'problem', problem })
  })

  it('reports network failures as an unreachable API', () => {
    expect(toApiFailure(new TypeError('Failed to fetch'))).toEqual({ kind: 'unreachable' })
  })

  it('reports anything else verbatim', () => {
    expect(toApiFailure(new Error('boom'))).toEqual({ kind: 'unexpected', message: 'boom' })
    expect(toApiFailure('Bad Gateway')).toEqual({ kind: 'unexpected', message: 'Bad Gateway' })
  })
})
