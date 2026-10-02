import { describe, expect, it } from 'vitest'
import { parseDocument } from '../usePublishWorkflow'

describe('parseDocument', () => {
  it('accepts JSON values', () => {
    expect(parseDocument('{"sites":[]}')).toEqual({ ok: true, value: { sites: [] } })
  })

  it('treats an empty document as incomplete rather than invalid', () => {
    expect(parseDocument('  \n')).toEqual({ ok: false, reason: '' })
  })

  it('reports why invalid JSON was rejected', () => {
    const result = parseDocument('{"sites":')
    expect(result.ok).toBe(false)
    expect(result.ok ? '' : result.reason).not.toBe('')
  })
})
