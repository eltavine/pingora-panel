/** The windows the traffic page offers, in seconds; the first is the default. */
export const WINDOWS = [3_600, 900, 21_600, 86_400, 604_800] as const

/** How often the page reads the traffic again. */
export const REFRESH_INTERVAL_MS = 30_000

const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const

/** Figures of the traffic page as people of `locale` read them. */
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

/** The largest value of every series, for a common scale; at least 1. */
export function scaleOf(series: readonly (readonly (number | null)[])[]): number {
  let largest = 0
  for (const values of series) {
    for (const value of values) {
      if (value !== null && Number.isFinite(value)) {
        largest = Math.max(largest, value)
      }
    }
  }
  return largest > 0 ? largest : 1
}

function points(
  values: readonly (number | null)[],
  scale: number,
  width: number,
  height: number,
): ([number, number] | null)[] {
  const step = values.length > 1 ? width / (values.length - 1) : 0
  return values.map((value, index) =>
    value === null || !Number.isFinite(value)
      ? null
      : [index * step, height - (Math.min(value, scale) / scale) * height],
  )
}

/** An SVG path through `values` in a `width` × `height` box; missing values break it. */
export function linePath(
  values: readonly (number | null)[],
  scale: number,
  width: number,
  height: number,
): string {
  let path = ''
  let drawing = false
  for (const point of points(values, scale, width, height)) {
    if (point === null) {
      drawing = false
      continue
    }
    path += `${drawing ? 'L' : 'M'}${point[0].toFixed(2)},${point[1].toFixed(2)}`
    drawing = true
  }
  return path
}

/** The area under `values` down to the bottom of the box, one shape per unbroken run. */
export function areaPath(
  values: readonly (number | null)[],
  scale: number,
  width: number,
  height: number,
): string {
  let path = ''
  let run: [number, number][] = []
  const close = () => {
    if (run.length > 1) {
      const first = run[0]!
      const last = run[run.length - 1]!
      path += `M${first[0].toFixed(2)},${height}`
      for (const [x, y] of run) {
        path += `L${x.toFixed(2)},${y.toFixed(2)}`
      }
      path += `L${last[0].toFixed(2)},${height}Z`
    }
    run = []
  }
  for (const point of points(values, scale, width, height)) {
    if (point === null) {
      close()
    } else {
      run.push(point)
    }
  }
  close()
  return path
}
