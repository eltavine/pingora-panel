import type { AlertMeasureName } from '@/api/generated'

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
