import type {
  CompressionAlgorithm,
  FieldChanges,
  HttpPolicy,
  HttpPolicyView,
  ServerHeader,
} from '@/api/generated'
import { lines, optionalNumber, parseSize, printSize, words } from '@/lib/forms'

export const OPERATIONS = ['set', 'add', 'remove'] as const
export type Operation = (typeof OPERATIONS)[number]
export const SERVER_MODES: readonly ServerHeader['mode'][] = ['keep', 'remove', 'replace']
/** The codings offered, by their HTTP names. */
export const CODINGS: readonly { algorithm: CompressionAlgorithm; name: string }[] = [
  { algorithm: 'gzip', name: 'gzip' },
  { algorithm: 'brotli', name: 'br' },
  { algorithm: 'zstd', name: 'zstd' },
]
/** What a new policy compresses: text, JSON, JavaScript, XML and SVG. */
export const DEFAULT_TYPES = [
  'text/*',
  'application/json',
  'application/javascript',
  'application/xml',
  'image/svg+xml',
]

/** RFC 9110 §5.6.2 token characters, which field names are made of. */
const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/

export interface FieldChangeForm {
  operation: Operation
  name: string
  value: string
}

export interface HttpPolicyForm {
  id: string
  request: FieldChangeForm[]
  response: FieldChangeForm[]
  server: ServerHeader['mode']
  serverValue: string
  cors: boolean
  origins: string
  methods: string
  headers: string
  expose: string
  credentials: boolean
  maxAge: number | string
  compression: boolean
  algorithms: CompressionAlgorithm[]
  types: string
  minSize: string
}

export function fieldChangeForm(): FieldChangeForm {
  return { operation: 'set', name: '', value: '' }
}

function changeForms(changes?: FieldChanges): FieldChangeForm[] {
  return [
    ...(changes?.remove ?? []).map((name) => ({ operation: 'remove' as const, name, value: '' })),
    ...(changes?.set ?? []).map((field) => ({ operation: 'set' as const, ...field })),
    ...(changes?.add ?? []).map((field) => ({ operation: 'add' as const, ...field })),
  ]
}

function fieldChanges(forms: readonly FieldChangeForm[]): FieldChanges {
  const named = forms.filter((form) => form.name.trim() !== '')
  const fields = (operation: Operation) =>
    named
      .filter((form) => form.operation === operation)
      .map((form) => ({ name: form.name.trim(), value: form.value.trim() }))
  return {
    remove: named.filter((form) => form.operation === 'remove').map((form) => form.name.trim()),
    set: fields('set'),
    add: fields('add'),
  }
}

export function httpPolicyForm(policy?: HttpPolicyView): HttpPolicyForm {
  const server = policy?.server ?? { mode: 'keep' }
  const minBytes = policy?.compression?.min_bytes ?? 0
  return {
    id: policy?.id ?? '',
    request: changeForms(policy?.request),
    response: changeForms(policy?.response),
    server: server.mode,
    serverValue: server.mode === 'replace' ? server.value : '',
    cors: Boolean(policy?.cors),
    origins: (policy?.cors?.allowed_origins ?? []).join('\n'),
    methods: (policy?.cors?.allowed_methods ?? []).join(' '),
    headers: (policy?.cors?.allowed_headers ?? []).join(' '),
    expose: (policy?.cors?.exposed_headers ?? []).join(' '),
    credentials: policy?.cors?.allow_credentials ?? false,
    maxAge: policy?.cors?.max_age_seconds ?? '',
    compression: Boolean(policy?.compression),
    algorithms: [...(policy?.compression?.algorithms ?? ['gzip', 'brotli'])],
    types: (policy?.compression?.types ?? DEFAULT_TYPES).join('\n'),
    minSize: policy?.compression ? (minBytes > 0 ? printSize(minBytes) : '') : '1k',
  }
}

export function httpPolicyBody(form: HttpPolicyForm): HttpPolicy {
  const server: ServerHeader =
    form.server === 'replace'
      ? { mode: 'replace', value: form.serverValue.trim() }
      : { mode: form.server }
  return {
    id: form.id.trim(),
    request: fieldChanges(form.request),
    response: fieldChanges(form.response),
    server,
    cors: form.cors
      ? {
          allowed_origins: lines(form.origins),
          allowed_methods: words(form.methods),
          allowed_headers: words(form.headers),
          exposed_headers: words(form.expose),
          allow_credentials: form.credentials,
          max_age_seconds: optionalNumber(form.maxAge),
        }
      : null,
    compression: form.compression
      ? {
          algorithms: CODINGS.map((coding) => coding.algorithm).filter((algorithm) =>
            form.algorithms.includes(algorithm),
          ),
          types: words(form.types),
          min_bytes: parseSize(form.minSize) ?? 0,
        }
      : null,
  }
}

/** Whether a change names a field: empty rows are left out, other names must be tokens. */
export function fieldNameInvalid(change: FieldChangeForm): boolean {
  const name = change.name.trim()
  return name !== '' && !TOKEN.test(name)
}

/** What a policy does, as message keys under `httpPolicies.effects`. */
export function effects(policy: HttpPolicy): string[] {
  const changes = (side?: FieldChanges) =>
    (side?.remove?.length ?? 0) + (side?.set?.length ?? 0) + (side?.add?.length ?? 0) > 0
  const checks: [boolean, string][] = [
    [changes(policy.request), 'request'],
    [changes(policy.response), 'response'],
    [(policy.server?.mode ?? 'keep') !== 'keep', 'server'],
    [Boolean(policy.cors), 'cors'],
    [Boolean(policy.compression), 'compression'],
  ]
  return checks.filter(([present]) => present).map(([, name]) => name)
}
