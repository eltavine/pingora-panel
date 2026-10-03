import type { ListenerView, TlsProfileView } from '@/api/generated'

export const TLS_VERSIONS = ['TLSv1.2', 'TLSv1.3'] as const
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
    alpn: [...form.alpn],
  }
}
