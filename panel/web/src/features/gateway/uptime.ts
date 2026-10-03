/** Seconds as the two largest whole units, such as `3d 4h` or `5m 12s`. */
export function formatUptime(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds))
  const units: [number, string][] = [
    [Math.floor(total / 86_400), 'd'],
    [Math.floor((total % 86_400) / 3_600), 'h'],
    [Math.floor((total % 3_600) / 60), 'm'],
    [total % 60, 's'],
  ]
  const first = units.findIndex(([value]) => value > 0)
  if (first === -1) {
    return '0s'
  }
  return units
    .slice(first, first + 2)
    .filter(([value], index) => index === 0 || value > 0)
    .map(([value, unit]) => `${value}${unit}`)
    .join(' ')
}
