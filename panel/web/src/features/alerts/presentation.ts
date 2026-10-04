import type {
  AlertComparisonName,
  AlertMeasureName,
  AlertNotificationView,
  AlertRuleSpecBody,
  AlertRuleView,
  AlertSeverityName,
} from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

export const MEASURES: readonly AlertMeasureName[] = [
  'server_error_ratio',
  'latency_p95',
  'request_rate',
  'upstream_error_ratio',
  'open_connections',
]

/** Pending periods offered, in seconds. */
export const PENDING_CHOICES = [0, 60, 300, 900, 3_600] as const

/** Measures sites and routes narrow. */
export function readsRequests(measure: AlertMeasureName): boolean {
  return measure === 'server_error_ratio' || measure === 'latency_p95' || measure === 'request_rate'
}

/** Shares the API keeps from 0 to 1 and people read as percentages. */
export function isRatio(measure: AlertMeasureName): boolean {
  return measure === 'server_error_ratio' || measure === 'upstream_error_ratio'
}

/** A threshold or reading as people read it: 5%, 250 ms, 30 req/s, 120. */
export function formatMeasure(
  measure: AlertMeasureName,
  value: number | null | undefined,
  locale: string,
): string {
  if (value === null || value === undefined) {
    return '—'
  }
  const number = (fraction: number) =>
    new Intl.NumberFormat(locale, { maximumFractionDigits: fraction }).format(value)
  switch (measure) {
    case 'server_error_ratio':
    case 'upstream_error_ratio':
      return new Intl.NumberFormat(locale, {
        style: 'percent',
        maximumFractionDigits: 2,
      }).format(value)
    case 'latency_p95':
      return value < 1
        ? `${new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(value * 1000)} ms`
        : `${number(2)} s`
    case 'request_rate':
      return `${number(2)} req/s`
    default:
      return number(0)
  }
}

/** Firing alerts read as negative and pending ones as warnings; disabled
 * rules are neither. */
export function toneOf(rule: AlertRuleView): StatusTone {
  if (!rule.spec.enabled) {
    return 'neutral'
  }
  switch (rule.state) {
    case 'firing':
      return 'negative'
    case 'pending':
      return 'warning'
    default:
      return 'positive'
  }
}

/** The message key of where a rule stands. */
export function stateKey(rule: AlertRuleView): string {
  return `alerts.states.${rule.spec.enabled ? rule.state : 'disabled'}`
}

export function notificationTone(notification: AlertNotificationView): StatusTone {
  switch (notification.state) {
    case 'delivered':
      return 'positive'
    case 'abandoned':
      return 'negative'
    default:
      return 'pending'
  }
}

/** A rule as the form edits it; the threshold in the unit people read. */
export interface RuleForm {
  id: string
  name: string
  description: string
  measure: AlertMeasureName
  comparison: AlertComparisonName
  threshold: number | string
  pendingSeconds: number
  site: string
  route: string
  upstream: string
  severity: AlertSeverityName
  enabled: boolean
  channels: string[]
}

export function ruleForm(rule?: AlertRuleView): RuleForm {
  const spec = rule?.spec
  const measure = spec?.measure ?? 'server_error_ratio'
  const threshold = spec?.threshold ?? 0.05
  return {
    id: rule?.id ?? '',
    name: spec?.name ?? '',
    description: spec?.description ?? '',
    measure,
    comparison: spec?.comparison ?? 'above',
    threshold: isRatio(measure) ? Number((threshold * 100).toFixed(4)) : threshold,
    pendingSeconds: spec?.pending_seconds ?? 300,
    site: spec?.site ?? '',
    route: spec?.route ?? '',
    upstream: spec?.upstream ?? '',
    severity: spec?.severity ?? 'warning',
    enabled: spec?.enabled ?? true,
    channels: [...(spec?.channels ?? [])],
  }
}

/** The rule the API keeps; scopes the measure does not read are dropped. */
export function ruleBody(form: RuleForm): AlertRuleSpecBody {
  const threshold = Number(form.threshold)
  const requests = readsRequests(form.measure)
  const site = requests ? form.site.trim() : ''
  return {
    name: form.name.trim() || form.id.trim(),
    description: form.description.trim(),
    measure: form.measure,
    comparison: form.comparison,
    threshold: isRatio(form.measure) ? threshold / 100 : threshold,
    pending_seconds: form.pendingSeconds,
    site: site || null,
    route: (site && form.route.trim()) || null,
    upstream: (form.measure === 'upstream_error_ratio' && form.upstream.trim()) || null,
    severity: form.severity,
    enabled: form.enabled,
    channels: form.channels,
  }
}
