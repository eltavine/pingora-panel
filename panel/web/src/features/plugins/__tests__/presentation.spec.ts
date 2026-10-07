import { describe, expect, it } from 'vitest'
import type { PluginView, VersionView } from '@/api/generated'
import {
  askedCapabilities,
  editorValue,
  fieldsOf,
  isReference,
  referenceChoices,
  settingsFrom,
  settingValue,
  stateTone,
  targetVersion,
  upgradeChoices,
} from '../presentation'

function version(number: string, overrides: Partial<VersionView> = {}): VersionView {
  return {
    version: number,
    publisher: 'Acme',
    description: 'DNS records',
    ports: ['dns01'],
    capabilities: ['dns01', 'secret-references'],
    protocol_versions: [1],
    compatible: true,
    problems: [],
    resources: {
      memory_bytes: 0,
      cpu_seconds: 0,
      open_files: 0,
      concurrency: 0,
      call_timeout_ms: 0,
    },
    executable_sha256: 'ab'.repeat(32),
    ...overrides,
  }
}

function plugin(overrides: Partial<PluginView> = {}): PluginView {
  return {
    name: 'dns',
    state: 'enabled',
    etag: '"3"',
    grants: [],
    settings: {},
    limits: { memory_bytes: 0, cpu_seconds: 0, open_files: 0, concurrency: 0, call_timeout_ms: 0 },
    versions: [version('1.0.0'), version('1.1.0'), version('2.0.0', { problems: ['unsigned'] })],
    active_version: '1.0.0',
    ...overrides,
  }
}

const schema = {
  type: 'object',
  required: ['zone'],
  properties: {
    zone: { type: 'string', title: 'Zone', description: 'The zone records go in' },
    token: { type: 'string', format: 'secret-reference' },
    ttl: { type: 'integer', minimum: 1, maximum: 86400 },
    ratio: { type: 'number' },
    dry_run: { type: 'boolean' },
    mode: { type: 'string', enum: ['fast', 'safe'] },
    servers: { type: 'array', items: { type: 'string' } },
    labels: { type: 'object' },
  },
}

describe('plugin presentation', () => {
  it('chooses versions by what can run', () => {
    expect(targetVersion(plugin())?.version).toBe('1.0.0')
    expect(targetVersion(plugin({ active_version: undefined }))?.version).toBe('1.1.0')
    expect(upgradeChoices(plugin()).map((choice) => choice.version)).toEqual(['1.1.0'])
    expect(stateTone('degraded')).toBe('negative')
    expect(stateTone('disabled')).toBe('neutral')
  })

  it('lists the capabilities asked for, ports first', () => {
    const asked = plugin({
      versions: [
        version('1.0.0', { capabilities: ['secret-references', 'notifications'] }),
        version('1.1.0', { capabilities: ['dns01'] }),
      ],
    })
    expect(askedCapabilities(asked)).toEqual(['dns01', 'notifications', 'secret-references'])
  })

  it('reads fields from a JSON Schema', () => {
    const fields = fieldsOf(schema)
    expect(fields.map((field) => [field.key, field.kind])).toEqual([
      ['zone', 'text'],
      ['token', 'secret'],
      ['ttl', 'integer'],
      ['ratio', 'number'],
      ['dry_run', 'boolean'],
      ['mode', 'choice'],
      ['servers', 'list'],
      ['labels', 'json'],
    ])
    expect(fields[0]).toMatchObject({ title: 'Zone', required: true })
    expect(fields[2]).toMatchObject({ minimum: 1, maximum: 86400, required: false })
    expect(fields[5]?.choices).toEqual(['fast', 'safe'])
    expect(fieldsOf({ type: 'string' })).toEqual([])
    expect(fieldsOf(undefined)).toEqual([])
  })

  it('turns edited fields into settings and back', () => {
    const fields = fieldsOf(schema)
    const byKey = Object.fromEntries(fields.map((field) => [field.key, field]))
    expect(editorValue(byKey.servers!, ['a', 'b'])).toBe('a\nb')
    expect(editorValue(byKey.labels!, { a: 1 })).toBe('{\n  "a": 1\n}')
    expect(editorValue(byKey.dry_run!, undefined)).toBe(false)
    expect(settingValue(byKey.ttl!, '300')).toBe(300)
    expect(settingValue(byKey.ttl!, 'soon')).toBe('soon')
    expect(settingValue(byKey.zone!, '  ')).toBeUndefined()
    expect(() => settingValue(byKey.labels!, '{')).toThrow(SyntaxError)

    const settings = settingsFrom(
      fields,
      {
        zone: 'example.com',
        token: 'vault:dns',
        ttl: '60',
        dry_run: false,
        servers: 'ns1.example.com\n\n ns2.example.com ',
        labels: '{"team": "edge"}',
      },
      { extra: true, dry_run: true },
    )
    expect(settings).toEqual({
      extra: true,
      zone: 'example.com',
      token: 'vault:dns',
      ttl: 60,
      dry_run: false,
      servers: ['ns1.example.com', 'ns2.example.com'],
      labels: { team: 'edge' },
    })
    expect(settingsFrom(fields, { dry_run: false }, {})).toEqual({})
  })

  it('suggests secret references', () => {
    expect(isReference('vault:dns-token')).toBe(true)
    expect(isReference('keeper:api/key')).toBe(true)
    expect(isReference('vault:')).toBe(false)
    expect(isReference('plain text')).toBe(false)
    const keeper = plugin({
      name: 'keeper',
      versions: [version('1.0.0', { ports: ['secrets'] })],
    })
    expect(
      referenceChoices(
        [{ name: 'dns-token', updated_at: '2026-10-07T00:00:00Z' }],
        [plugin(), keeper],
        'dns',
      ),
    ).toEqual(['vault:dns-token', 'keeper:'])
    expect(referenceChoices([], [keeper], 'keeper')).toEqual([])
  })
})
