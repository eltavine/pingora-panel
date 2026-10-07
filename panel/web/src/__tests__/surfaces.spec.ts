import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import router from '@/router'

interface Surfaces {
  operations: Record<
    string,
    { cli?: string; console?: string; console_via?: string; specific?: string }
  >
}
interface Contract {
  paths: Record<string, Record<string, { operationId?: string }>>
}

// The global URL is the DOM's here, so paths are joined as strings.
const read = <T>(path: string): T =>
  JSON.parse(readFileSync(join(import.meta.dirname, path), 'utf8')) as T
const surfaces = read<Surfaces>('../../../surfaces.json')
const contract = read<Contract>('../../../panel-api/tests/fixtures/openapi.json')

const paths = new Map(
  Object.entries(contract.paths).flatMap(([path, item]) =>
    Object.values(item).flatMap((operation) =>
      operation.operationId ? [[operation.operationId, path] as const] : [],
    ),
  ),
)

/** The console's own code: no generated client, mocks or tests. */
function sources(directory: string): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name)
    if (statSync(path).isDirectory()) {
      return ['__tests__', 'generated', 'mocks'].includes(name) ? [] : sources(path)
    }
    return /\.(ts|vue)$/.test(name) ? [readFileSync(path, 'utf8')] : []
  })
}
const code = sources(join(import.meta.dirname, '..'))
const imported = new Set(
  code.flatMap((text) =>
    [...text.matchAll(/import\s*\{([^}]*)\}\s*from\s*'@\/api\/generated'/g)].flatMap((match) =>
      match[1]!.split(',').map((name) => name.trim().split(/\s+as\s+/)[0]!),
    ),
  ),
)

const camel = (id: string) =>
  id.replace(/_([a-z0-9])/g, (_, letter: string) => letter.toUpperCase())
const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** Whether the console calls `operation`: through its generated query or
 * mutation, the client function it imports, or the operation's path. */
function called(operation: string): boolean {
  const name = camel(operation)
  const symbol = new RegExp(`\\b${name}(Options|Mutation|InfiniteOptions|QueryKey)\\b`)
  const path = paths.get(operation)!
  const literal = new RegExp(
    path
      .split('/')
      .map((part) => (part.startsWith('{') ? '\\$\\{[^}]+\\}' : escape(part)))
      .join('/'),
  )
  return imported.has(name) || code.some((text) => symbol.test(text) || literal.test(text))
}

describe('surface parity (ADR 0045)', () => {
  const routes = new Set(router.getRoutes().map((route) => route.path))
  const declared = Object.entries(surfaces.operations).filter(([, entry]) => entry.console)

  it('names console routes that exist', () => {
    const missing = declared
      .filter(([, entry]) => !routes.has(entry.console!))
      .map(([operation, entry]) => `${operation}: ${entry.console}`)
    expect(missing).toEqual([])
  })

  it('calls every operation it is said to offer', () => {
    const missing = declared
      .filter(([operation, entry]) => !called(entry.console_via ?? operation))
      .map(([operation, entry]) => entry.console_via ?? operation)
    expect(missing).toEqual([])
  })
})
