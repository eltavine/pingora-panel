import type { CertificateStatus } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

export const STATUS_TONES: Record<CertificateStatus, StatusTone> = {
  valid: 'positive',
  expiring: 'warning',
  expired: 'negative',
  not_yet_valid: 'pending',
}
