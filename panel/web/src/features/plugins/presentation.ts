import type { Component } from 'vue'
import { Archive, Bell, Container, Globe, KeyRound, Network, Puzzle } from '@lucide/vue'
import type { PluginState, PluginView, SecretView, VersionView } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** The capability that lets a plugin receive the secrets its settings name. */
export const SECRET_REFERENCES = 'secret-references'

/** The JSON Schema format of settings that name a secret. */
export const SECRET_REFERENCE_FORMAT = 'secret-reference'

const PORT_ICONS: Record<string, Component> = {
  dns01: Globe,
  secrets: KeyRound,
  notifications: Bell,
  backups: Archive,
  containers: Container,
  gateway: Network,
}

/** The icon of a port, or of a capability named after one. */
export function portIcon(port: string): Component {
  return PORT_ICONS[port] ?? Puzzle
}

export function stateTone(state: PluginState): StatusTone {
  switch (state) {
    case 'enabled':
      return 'positive'
    case 'degraded':
      return 'negative'
    default:
      return 'neutral'
  }
}

export function canRun(version: VersionView): boolean {
  return version.problems.length === 0
}

/** The version settings and limits are checked against: the active one,
 * or else the newest that can run. */
export function targetVersion(plugin: PluginView): VersionView | undefined {
  return (
    plugin.versions.find((version) => version.version === plugin.active_version) ??
    plugin.versions.filter(canRun).at(-1)
  )
}

/** The versions an upgrade can choose: those that can run, but the active one. */
export function upgradeChoices(plugin: PluginView): VersionView[] {
  return plugin.versions.filter(
    (version) => canRun(version) && version.version !== plugin.active_version,
  )
}

/** Every capability a version asks for, ports first, then the rest by name. */
export function askedCapabilities(plugin: PluginView): string[] {
  const asked = new Set(plugin.versions.flatMap((version) => version.capabilities))
  const ports = [...asked].filter((capability) => capability in PORT_ICONS)
  const others = [...asked].filter((capability) => !(capability in PORT_ICONS)).sort()
  return [...ports.sort(), ...others]
}

/** The ports of the version that runs, or would. */
export function portsOf(plugin: PluginView): string[] {
  return targetVersion(plugin)?.ports ?? []
}

export type FieldKind =
  'text' | 'secret' | 'integer' | 'number' | 'boolean' | 'choice' | 'list' | 'json'

/** A setting as its editor shows it. */
export interface Field {
  key: string
  kind: FieldKind
  title: string
  description?: string
  required: boolean
  choices?: string[]
  minimum?: number
  maximum?: number
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function kindOf(property: Record<string, unknown>): FieldKind {
  const type = Array.isArray(property.type)
    ? property.type.find((type) => type !== 'null')
    : property.type
  if (Array.isArray(property.enum) && property.enum.every((item) => typeof item === 'string')) {
    return 'choice'
  }
  switch (type) {
    case 'string':
      return property.format === SECRET_REFERENCE_FORMAT ? 'secret' : 'text'
    case 'integer':
      return 'integer'
    case 'number':
      return 'number'
    case 'boolean':
      return 'boolean'
    case 'array':
      return isRecord(property.items) &&
        property.items.type === 'string' &&
        property.items.format !== SECRET_REFERENCE_FORMAT
        ? 'list'
        : 'json'
    default:
      return 'json'
  }
}

/** The fields a JSON Schema's top-level properties describe, in their order. */
export function fieldsOf(schema: unknown): Field[] {
  if (!isRecord(schema) || !isRecord(schema.properties)) {
    return []
  }
  const required = new Set(Array.isArray(schema.required) ? schema.required : [])
  return Object.entries(schema.properties)
    .filter((entry): entry is [string, Record<string, unknown>] => isRecord(entry[1]))
    .map(([key, property]) => {
      const kind = kindOf(property)
      return {
        key,
        kind,
        title: typeof property.title === 'string' ? property.title : key,
        description: typeof property.description === 'string' ? property.description : undefined,
        required: required.has(key),
        choices: kind === 'choice' ? (property.enum as string[]) : undefined,
        minimum: typeof property.minimum === 'number' ? property.minimum : undefined,
        maximum: typeof property.maximum === 'number' ? property.maximum : undefined,
      }
    })
}

/** What a field's editor shows for a setting's value. */
export function editorValue(field: Field, value: unknown): string | boolean {
  switch (field.kind) {
    case 'boolean':
      return value === true
    case 'list':
      return Array.isArray(value) ? value.map(String).join('\n') : ''
    case 'json':
      return value === undefined ? '' : JSON.stringify(value, null, 2)
    default:
      return value === undefined || value === null ? '' : String(value)
  }
}

/** A field's edited value as settings hold it; `undefined` leaves the
 * setting out. Unreadable JSON throws a `SyntaxError`. */
export function settingValue(field: Field, edited: string | boolean): unknown {
  if (field.kind === 'boolean') {
    return edited === true
  }
  const text = String(edited)
  if (!text.trim()) {
    return undefined
  }
  switch (field.kind) {
    case 'integer':
    case 'number': {
      const number = Number(text)
      return Number.isFinite(number) ? number : text
    }
    case 'list':
      return text
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
    case 'json':
      return JSON.parse(text)
    default:
      return text
  }
}

/** The settings the edited fields make, keeping settings no field shows. */
export function settingsFrom(
  fields: Field[],
  edited: Record<string, string | boolean>,
  original: Record<string, unknown>,
): Record<string, unknown> {
  const shown = new Set(fields.map((field) => field.key))
  const settings: Record<string, unknown> = Object.fromEntries(
    Object.entries(original).filter(([key]) => !shown.has(key)),
  )
  for (const field of fields) {
    const value = settingValue(field, edited[field.key] ?? '')
    if (
      value !== undefined &&
      !(field.kind === 'boolean' && value === false && !(field.key in original))
    ) {
      settings[field.key] = value
    }
  }
  return settings
}

/** Whether `text` names a secret: `vault:<name>` or `<plugin>:<path>`. */
export function isReference(text: string): boolean {
  return /^vault:[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(text) || /^[a-z0-9][a-z0-9-]*:.+$/.test(text)
}

/** References a secret setting can take: the kept secrets, then the other
 * plugins that provide secrets, each to be completed with a path. */
export function referenceChoices(
  secrets: SecretView[],
  plugins: PluginView[],
  self: string,
): string[] {
  const providers = plugins
    .filter((plugin) => plugin.name !== self && portsOf(plugin).includes('secrets'))
    .map((plugin) => `${plugin.name}:`)
  return [...secrets.map((secret) => `vault:${secret.name}`), ...providers]
}
