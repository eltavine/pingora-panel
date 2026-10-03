import type { ListenerView, TlsProfileView } from '@/api/generated'

export const TLS_VERSIONS = ['TLSv1.2', 'TLSv1.3'] as const
/** The maximum version when none is named. */
export const NEWEST = 'newest'
/** Cipher suites a profile may name, by IANA name and TLS version. */
export const CIPHER_SUITES = {
  'TLSv1.3': [
    'TLS13_AES_256_GCM_SHA384',
    'TLS13_AES_128_GCM_SHA256',
    'TLS13_CHACHA20_POLY1305_SHA256',
  ],
  'TLSv1.2': [
    'TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384',
    'TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256',
    'TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256',
    'TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384',
    'TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256',
    'TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256',
  ],
} as const
export const ALPN_PROTOCOLS = ['h2', 'http/1.1'] as const
export const RESOURCE_ID = /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/

export interface ListenerForm {
  id: string
  address: string
  http1: boolean
  http2: boolean
  http3: boolean
  reusePort: boolean
  ipv6Only: boolean
  tlsProfileId: string
  defaultSiteId: string
}

/** Where a profile's certificate comes from. */
export type CertificateSource = 'inventory' | 'files'

export interface TlsProfileForm {
  id: string
  source: CertificateSource
  certificateId: string
  certificateSecretId: string
  privateKeySecretId: string
  minProtocol: string
  /** {@link NEWEST} names no maximum. */
  maxProtocol: string
  cipherSuites: string[]
  sessionResumption: boolean
  ocspStapling: boolean
  alpn: string[]
}

export function listenerForm(listener?: ListenerView): ListenerForm {
  return {
    id: listener?.id ?? '',
    address: listener?.address ?? '0.0.0.0:80',
    http1: listener?.protocols?.http1 ?? true,
    http2: listener?.protocols?.http2 ?? false,
    http3: listener?.protocols?.http3 ?? false,
    reusePort: listener?.reuse_port ?? false,
    ipv6Only: listener?.ipv6_only ?? false,
    tlsProfileId: listener?.tls_profile_id ?? '',
    defaultSiteId: listener?.default_site_id ?? '',
  }
}

export function listenerBody(form: ListenerForm) {
  return {
    id: form.id.trim(),
    address: form.address.trim(),
    protocols: { http1: form.http1, http2: form.http2, http3: form.http3 },
    reuse_port: form.reusePort,
    ipv6_only: form.ipv6Only ? true : null,
    tls_profile_id: form.tlsProfileId || null,
    default_site_id: form.defaultSiteId || null,
  }
}

export function tlsProfileForm(profile?: TlsProfileView): TlsProfileForm {
  return {
    id: profile?.id ?? '',
    source: profile && !profile.certificate_id ? 'files' : 'inventory',
    certificateId: profile?.certificate_id ?? '',
    certificateSecretId: profile?.certificate_secret_id ?? '',
    privateKeySecretId: profile?.private_key_secret_id ?? '',
    minProtocol: profile?.min_protocol ?? 'TLSv1.2',
    maxProtocol: profile?.max_protocol ?? NEWEST,
    cipherSuites: [...(profile?.cipher_suites ?? [])],
    sessionResumption: profile?.session_resumption ?? true,
    ocspStapling: profile?.ocsp_stapling ?? false,
    alpn: [...(profile?.alpn ?? ['h2', 'http/1.1'])],
  }
}

export function tlsProfileBody(form: TlsProfileForm) {
  const certificate =
    form.source === 'inventory'
      ? { certificate_id: form.certificateId }
      : {
          certificate_secret_id: form.certificateSecretId.trim(),
          private_key_secret_id: form.privateKeySecretId.trim(),
        }
  return {
    id: form.id.trim(),
    ...certificate,
    min_protocol: form.minProtocol,
    max_protocol: form.maxProtocol === NEWEST ? null : form.maxProtocol,
    cipher_suites: [...form.cipherSuites],
    session_resumption: form.sessionResumption,
    ocsp_stapling: form.ocspStapling,
    alpn: [...form.alpn],
  }
}
