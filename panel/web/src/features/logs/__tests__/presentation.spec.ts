import { describe, expect, it } from 'vitest'
import type { LogRecordItem } from '@/api/generated'
import { downloadUrl, queryOf, tailUrl, toneOf } from '../presentation'
import { summaryOf } from '@/lib/logs'

function record(overrides: Partial<LogRecordItem>): LogRecordItem {
  return {
    time: '2026-10-04T10:00:00.000000005Z',
    kind: 'access',
    line: '{}',
    fields: {},
    ...overrides,
  }
}

describe('log records', () => {
  it('read server errors as negative and client errors as warnings', () => {
    expect(toneOf(record({ status: 502 }))).toBe('negative')
    expect(toneOf(record({ status: 404 }))).toBe('warning')
    expect(toneOf(record({ status: 200 }))).toBe('positive')
    expect(toneOf(record({}))).toBe('neutral')
    expect(toneOf(record({ kind: 'error' }))).toBe('negative')
  })

  it('sum up requests and error messages', () => {
    expect(summaryOf(record({ method: 'GET', path: '/cart' }))).toBe('GET /cart')
    expect(
      summaryOf(record({ kind: 'error', line: '{"message":"upstream refused the connection"}' })),
    ).toBe('upstream refused the connection')
    expect(summaryOf(record({ kind: 'error', line: 'plain', fields: { message: 'kept' } }))).toBe(
      'kept',
    )
    expect(summaryOf(record({ kind: 'error', line: 'plain' }))).toBe('plain')
  })
})

describe('log locations', () => {
  const sameOrigin = { baseUrl: '', page: 'https://panel.example/logs?site=shop' }

  it('keep only the filters that are set', () => {
    expect(queryOf({ site: 'shop', status: '', text: 'boom' })).toEqual({
      site: 'shop',
      text: 'boom',
    })
  })

  it('follow over the WebSocket scheme of the API origin', () => {
    expect(tailUrl({ site: 'shop', after: '2026-10-04T10:00:00Z' }, sameOrigin)).toBe(
      'wss://panel.example/api/v1/logs/tail?site=shop&after=2026-10-04T10%3A00%3A00Z',
    )
    expect(tailUrl({}, { baseUrl: 'http://127.0.0.1:8080', page: sameOrigin.page })).toBe(
      'ws://127.0.0.1:8080/api/v1/logs/tail',
    )
  })

  it('download what the filters match', () => {
    expect(downloadUrl({ kind: 'error' }, sameOrigin)).toBe(
      'https://panel.example/api/v1/logs/download?kind=error',
    )
  })
})
