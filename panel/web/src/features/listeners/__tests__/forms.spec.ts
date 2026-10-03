import { describe, expect, it } from 'vitest'
import { RESOURCE_ID, listenerBody, listenerForm, tlsProfileBody, tlsProfileForm } from '../forms'

describe('listener forms', () => {
  it('leaves IPv6-only and empty references unset', () => {
    const form = listenerForm()
    form.id = 'public-http'
    expect(listenerBody(form)).toEqual({
      id: 'public-http',
      address: '0.0.0.0:80',
      protocols: { http1: true, http2: false, http3: false },
      reuse_port: false,
      ipv6_only: null,
      tls_profile_id: null,
      default_site_id: null,
    })
  })

  it('accepts lowercase identifiers only', () => {
    expect(RESOURCE_ID.test('edge-443')).toBe(true)
    expect(RESOURCE_ID.test('Edge')).toBe(false)
    expect(RESOURCE_ID.test('-edge')).toBe(false)
    expect(RESOURCE_ID.test('edge-')).toBe(false)
  })
})

describe('TLS profile forms', () => {
  it('defaults to TLS 1.2 with both ALPN protocols', () => {
    const form = tlsProfileForm()
    form.id = 'example'
    form.source = 'files'
    form.certificateSecretId = ' example.crt '
    form.privateKeySecretId = 'example.key'
    expect(tlsProfileBody(form)).toEqual({
      id: 'example',
      certificate_secret_id: 'example.crt',
      private_key_secret_id: 'example.key',
      min_protocol: 'TLSv1.2',
      alpn: ['h2', 'http/1.1'],
    })
  })

  it('names a certificate of the inventory or files, never both', () => {
    const form = tlsProfileForm()
    expect(form.source).toBe('inventory')
    form.id = 'edge'
    form.certificateId = 'example.com'
    form.certificateSecretId = 'left-over.crt'
    expect(tlsProfileBody(form)).toEqual({
      id: 'edge',
      certificate_id: 'example.com',
      min_protocol: 'TLSv1.2',
      alpn: ['h2', 'http/1.1'],
    })
    const files = tlsProfileForm({
      id: 'files',
      certificate_secret_id: 'site.crt',
      private_key_secret_id: 'site.key',
      min_protocol: 'TLSv1.3',
      alpn: [],
      etag: '"1"',
    })
    expect(files.source).toBe('files')
    const managed = tlsProfileForm({
      id: 'managed',
      certificate_id: 'example.com',
      certificate_secret_id: '',
      private_key_secret_id: '',
      min_protocol: 'TLSv1.2',
      alpn: [],
      etag: '"1"',
    })
    expect(managed.source).toBe('inventory')
  })
})
