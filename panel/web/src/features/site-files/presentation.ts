/** The largest file the console edits as text. */
export const MOST_TEXT_BYTES = 1024 * 1024

/** Extensions of files the console edits as text. */
const TEXT = new Set([
  'html',
  'htm',
  'css',
  'js',
  'mjs',
  'cjs',
  'json',
  'map',
  'txt',
  'md',
  'xml',
  'svg',
  'yaml',
  'yml',
  'toml',
  'ini',
  'conf',
  'csv',
  'webmanifest',
])

/** `name` inside `parent`, or `name` at the top. */
export function childPath(parent: string, name: string): string {
  return parent ? `${parent}/${name}` : name
}

/** The directory above `path`; empty at the top. */
export function parentPath(path: string): string {
  const at = path.lastIndexOf('/')
  return at < 0 ? '' : path.slice(0, at)
}

/** The directories down to `path`, outermost first, each with its path. */
export function crumbs(path: string): { name: string; path: string }[] {
  const parts = path.split('/').filter(Boolean)
  return parts.map((name, index) => ({ name, path: parts.slice(0, index + 1).join('/') }))
}

/** A path as the address keeps it, without leading or trailing slashes. */
export function normalizedPath(value: unknown): string {
  return typeof value === 'string' ? value.replace(/^\/+|\/+$/g, '') : ''
}

/** Whether a file of this name and size is edited as text here. */
export function editable(name: string, size: number): boolean {
  const dot = name.lastIndexOf('.')
  const extension = dot < 0 ? '' : name.slice(dot + 1).toLowerCase()
  return size <= MOST_TEXT_BYTES && (TEXT.has(extension) || !extension)
}
