const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const

/** Figures as people of `locale` read them. */
export function formatters(locale: string) {
  const whole = new Intl.NumberFormat(locale, { maximumFractionDigits: 0 })
  const fraction = new Intl.NumberFormat(locale, { maximumFractionDigits: 2 })
  const percent = new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 1 })
  return {
    count: (value: number) => whole.format(value),
    rate: (value: number) => fraction.format(value),
    percent: (ratio: number) => percent.format(ratio),
    /** Latency in seconds: milliseconds below a second; a dash when unmeasured. */
    seconds: (value: number | null | undefined) =>
      value === null || value === undefined
        ? '—'
        : value < 1
          ? `${whole.format(value * 1_000)} ms`
          : `${fraction.format(value)} s`,
    bytes: (value: number) => {
      let size = value
      let unit = 0
      while (size >= 1_024 && unit < BYTE_UNITS.length - 1) {
        size /= 1_024
        unit += 1
      }
      return `${fraction.format(size)} ${BYTE_UNITS[unit]}`
    },
  }
}

/** A node's address and port, with brackets around IPv6 addresses. */
export function nodeOf(node: { address: string; port: number }): string {
  return node.address.includes(':')
    ? `[${node.address}]:${node.port}`
    : `${node.address}:${node.port}`
}
