import type { CertificateStatus, KeyAlgorithm } from '@/api/generated'
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

export const KEY_ALGORITHMS: Record<KeyAlgorithm, string> = {
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
