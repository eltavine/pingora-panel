/** The file every configuration starts from. */
export const ENTRY = 'main.conf'

/** A relative `.conf` path whose segments are plain names, so never `.` or `..`. */
export function isFilePath(path: string): boolean {
  return (
    path.endsWith('.conf') &&
    path.split('/').every((segment) => /^[A-Za-z0-9_][A-Za-z0-9._-]*$/.test(segment))
  )
}

/** `main.conf` first, then the other files by path. */
export function sortPaths(paths: Iterable<string>): string[] {
  return [...paths].sort((left, right) =>
    left === ENTRY ? -1 : right === ENTRY ? 1 : left.localeCompare(right),
  )
}
