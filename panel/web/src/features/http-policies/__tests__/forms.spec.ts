import { describe, expect, it } from 'vitest'
import type { HttpPolicyView } from '@/api/generated'
import { effects, fieldNameInvalid, httpPolicyBody, httpPolicyForm } from '../forms'

const policy: HttpPolicyView = {
  id: 'api',
  request: {
    remove: ['X-Internal'],
    set: [{ name: 'X-Tenant', value: '$host' }],
    add: [],
  },
  response: { add: [{ name: 'Link', value: '</app.css>; rel=preload' }] },
  server: { mode: 'replace', value: 'shop' },
  cors: {
    allowed_origins: ['https://*.shop.example', 'https://admin.shop.example'],
    allowed_methods: ['PUT', 'DELETE'],
    allowed_headers: ['X-Api-Key'],
    exposed_headers: [],
    allow_credentials: true,
    max_age_seconds: 600,
  },
  compression: { algorithms: ['brotli', 'gzip'], types: ['text/*'], min_bytes: 1024 },
  used_by: [],
  etag: '"h1"',
}

describe('HTTP policy forms', () => {
  it('round-trip a policy', () => {
    const form = httpPolicyForm(policy)
    expect(form.request.map((change) => change.operation)).toEqual(['remove', 'set'])
    expect(form.server).toBe('replace')
    expect(form.minSize).toBe('1k')
    const body = httpPolicyBody(form)
    expect(body).toEqual({
      id: 'api',
      request: {
        remove: ['X-Internal'],
        set: [{ name: 'X-Tenant', value: '$host' }],
        add: [],
      },
      response: { remove: [], set: [], add: [{ name: 'Link', value: '</app.css>; rel=preload' }] },
      server: { mode: 'replace', value: 'shop' },
      cors: policy.cors,
      compression: { algorithms: ['gzip', 'brotli'], types: ['text/*'], min_bytes: 1024 },
    })
  })

  it('start new policies plain and leave out what is off or empty', () => {
    const form = httpPolicyForm()
    expect(form.server).toBe('keep')
    expect(form.types.split('\n')).toContain('application/json')
    form.id = 'plain'
    form.request.push({ operation: 'remove', name: ' ', value: '' })
    form.server = 'remove'
    expect(httpPolicyBody(form)).toMatchObject({
      request: { remove: [], set: [], add: [] },
      server: { mode: 'remove' },
      cors: null,
      compression: null,
    })
    form.compression = true
    form.minSize = ''
    expect(httpPolicyBody(form).compression?.min_bytes).toBe(0)
  })

  it('flag field names that are not tokens and summarize effects', () => {
    expect(fieldNameInvalid({ operation: 'set', name: 'X Frame', value: '' })).toBe(true)
    expect(fieldNameInvalid({ operation: 'set', name: 'X-Frame-Options', value: '' })).toBe(false)
    expect(fieldNameInvalid({ operation: 'set', name: '', value: '' })).toBe(false)
    expect(effects(policy)).toEqual(['request', 'response', 'server', 'cors', 'compression'])
    expect(effects({ id: 'none', server: { mode: 'keep' } })).toEqual([])
  })
})
