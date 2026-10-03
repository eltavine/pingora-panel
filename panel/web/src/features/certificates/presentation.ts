import type { CertificateStatus, IssuanceState, KeyAlgorithm } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** Lowercase letters, digits and hyphens in labels separated by dots. */
export const CERTIFICATE_ID =
  /^(?=.{1,64}$)[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)*$/

export const MAX_SELF_SIGNED_DAYS = 825

const DAY = 24 * 60 * 60 * 1000

export const STATUS_TONES: Record<CertificateStatus, StatusTone> = {
  valid: 'positive',
  expiring: 'warning',
  expired: 'negative',
  not_yet_valid: 'pending',
}

/** Names of the key algorithms the panel knows; others show as they are. */
export const KEY_ALGORITHMS: Partial<Record<KeyAlgorithm, string>> = {
  rsa: 'RSA',
  ecdsa_p256: 'ECDSA P-256',
  ecdsa_p384: 'ECDSA P-384',
  ed25519: 'Ed25519',
}

/** A SHA-256 fingerprint as browsers show it, `AB:CD:…`. */
export function fingerprint(hex: string): string {
  return (hex.match(/.{1,2}/g) ?? []).join(':').toUpperCase()
}

/** Whole days until `notAfter`; negative once it has passed. */
export function daysLeft(notAfter: string, now = Date.now()): number {
  return Math.floor((Date.parse(notAfter) - now) / DAY)
}

/** Names typed one per line, or separated by commas or spaces. */
export function parseNames(text: string): string[] {
  return [
    ...new Set(
      text
        .split(/[\s,]+/)
        .map((name) => name.trim())
        .filter(Boolean),
    ),
  ]
}

/** An identifier suggested by the first DNS name, such as `example.com`. */
export function suggestedId(names: readonly string[]): string {
  const name = names.find((candidate) => !/^[\d.:]+$/.test(candidate)) ?? names[0] ?? ''
  const id = name.replace(/^\*\./, '').toLowerCase()
  return CERTIFICATE_ID.test(id) ? id : ''
}

/** Lowercase letters, digits and hyphens, such as `letsencrypt`. */
export const ACCOUNT_ID = /^(?=.{1,64}$)[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/

/** A well-known public ACME CA. */
export interface AcmeDirectory {
  readonly id: 'letsencrypt' | 'letsencrypt-staging' | 'zerossl' | 'google'
  readonly url: string
  readonly terms: string
  /** Registering needs an external account binding from the CA. */
  readonly binding: boolean
}

export const ACME_DIRECTORIES: readonly AcmeDirectory[] = [
  {
    id: 'letsencrypt',
    url: 'https://acme-v02.api.letsencrypt.org/directory',
    terms: 'https://letsencrypt.org/repository/',
    binding: false,
  },
  {
    id: 'letsencrypt-staging',
    url: 'https://acme-staging-v02.api.letsencrypt.org/directory',
    terms: 'https://letsencrypt.org/repository/',
    binding: false,
  },
  {
    id: 'zerossl',
    url: 'https://acme.zerossl.com/v2/DV90',
    terms: 'https://zerossl.com/terms/',
    binding: true,
  },
  {
    id: 'google',
    url: 'https://dv.acme-v02.api.pki.goog/directory',
    terms: 'https://pki.goog/repository/',
    binding: true,
  },
]

/** The well-known CA a directory URL belongs to. */
export function knownDirectory(url: string): AcmeDirectory | undefined {
  return ACME_DIRECTORIES.find((directory) => directory.url === url)
}

/** The host of a directory URL. */
export function directoryHost(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

export const ISSUANCE_TONES: Record<IssuanceState, StatusTone> = {
  pending: 'pending',
  issued: 'positive',
  failing: 'negative',
}
