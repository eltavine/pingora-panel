import { sample } from 'openapi-sampler'
import contractUrl from '../../../panel-api/tests/fixtures/openapi.json?url'

interface Operation {
  responses?: Record<string, { content?: Record<string, { schema?: object }> }>
}

interface Contract {
  paths: Record<string, Partial<Record<string, Operation>>>
}

interface Route {
  method: string
  pattern: RegExp
  /** Static path segments; a more specific route answers first. */
  statics: number
  operation: Operation
}

const METHODS = ['get', 'put', 'post', 'patch', 'delete'] as const

const TIME_KEY = /(_at|_until|^since|^until|time)$/

/**
 * Times the contract types as plain strings, made real: a sampled string
 * where a time belongs is a moment of the last day.
 */
function withTimes(value: unknown, key = ''): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => withTimes(item, key))
  }
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([name, item]) => [name, withTimes(item, name)]),
    )
  }
  if (typeof value === 'string' && TIME_KEY.test(key) && Number.isNaN(Date.parse(value))) {
    return new Date(Date.now() - Math.floor(Math.random() * 86_400_000)).toISOString()
  }
  return value
}

/** Answers shaped by the reviewed OpenAPI contract of the management API. */
export interface Sampler {
  /** A response to `method` `path`, or `undefined` when the contract has no such operation. */
  respond(method: string, path: string): { status: number; body: unknown } | undefined
  /** A value shaped like the contract's component schema `name`. */
  schema<T>(name: string): T
}

export async function loadSampler(): Promise<Sampler> {
  const contract = (await (await fetch(contractUrl)).json()) as Contract
  const routes: Route[] = Object.entries(contract.paths)
    .flatMap(([template, item]) =>
      METHODS.flatMap((method) => {
        const operation = item[method]
        return operation
          ? [
              {
                method: method.toUpperCase(),
                pattern: new RegExp(`^${template.replace(/\{[^}]+\}/g, '[^/]+')}$`),
                statics: template.split('/').filter((part) => part && !part.startsWith('{')).length,
                operation,
              },
            ]
          : []
      }),
    )
    .sort((left, right) => right.statics - left.statics)
  const sampled = (schema: object) => withTimes(sample(schema, { skipReadOnly: false }, contract))
  return {
    respond(method, path) {
      const route = routes.find(
        (candidate) => candidate.method === method && candidate.pattern.test(path),
      )
      if (!route) {
        return undefined
      }
      const [status, response] = Object.entries(route.operation.responses ?? {}).find(([code]) =>
        code.startsWith('2'),
      ) ?? ['200', {}]
      const schema = response.content?.['application/json']?.schema
      return { status: Number(status), body: schema ? sampled(schema) : undefined }
    },
    schema<T>(name: string) {
      return sampled({ $ref: `#/components/schemas/${name}` }) as T
    },
  }
}
