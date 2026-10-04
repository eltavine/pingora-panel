import { describe, expect, it } from 'vitest'
import { NEWEST, listenerBody, listenerForm, tlsProfileBody, tlsProfileForm } from '../forms'
import { RESOURCE_ID } from '@/lib/forms'

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
      trusted_proxies: [],
      real_ip_header: 'x-forwarded-for',
      request_head_timeout_seconds: null,
    })
    form.trustedProxies = '10.0.0.0/8\n 192.0.2.7, '
    form.realIpHeader = 'forwarded'
    form.requestHeadTimeout = 15
    expect(listenerBody(form)).toMatchObject({
      trusted_proxies: ['10.0.0.0/8', '192.0.2.7'],
      real_ip_header: 'forwarded',
      request_head_timeout_seconds: 15,
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
      max_protocol: null,
      cipher_suites: [],
      session_resumption: true,
      ocsp_stapling: false,
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
      max_protocol: null,
      cipher_suites: [],
      session_resumption: true,
      ocsp_stapling: false,
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

  it('caps versions, names cipher suites and turns resumption off', () => {
    const form = tlsProfileForm()
    form.id = 'strict'
    form.certificateId = 'example.com'
    form.maxProtocol = 'TLSv1.2'
    form.cipherSuites = ['TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256']
    form.sessionResumption = false
    expect(tlsProfileBody(form)).toMatchObject({
      max_protocol: 'TLSv1.2',
      cipher_suites: ['TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256'],
      session_resumption: false,
    })
    const saved = tlsProfileForm({
      id: 'strict',
      certificate_secret_id: '',
      private_key_secret_id: '',
      min_protocol: 'TLSv1.2',
      max_protocol: 'TLSv1.2',
      cipher_suites: ['TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256'],
      session_resumption: false,
      alpn: [],
      etag: '"1"',
    })
    expect([saved.maxProtocol, saved.sessionResumption]).toEqual(['TLSv1.2', false])
    expect(tlsProfileForm().maxProtocol).toBe(NEWEST)
  })
})
