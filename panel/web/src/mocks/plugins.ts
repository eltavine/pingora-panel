import { http, HttpResponse, type AnyHandler } from 'msw'
import type {
  NewTrustedKey,
  PluginGrants,
  PluginLimits,
  PluginView,
  SecretView,
  TrustedKeyView,
  VersionView,
} from '@/api/generated'

const PORTS = ['dns01', 'secrets', 'notifications', 'backups', 'containers', 'gateway']

const SCHEMA = {
  type: 'object',
  properties: {
    zone: { type: 'string', title: 'Zone', description: 'The DNS zone records are published in' },
    token: { type: 'string', format: 'secret-reference', title: 'API token' },
    ttl: { type: 'integer', minimum: 30, maximum: 86400, title: 'Record TTL' },
    dry_run: { type: 'boolean', title: 'Dry run' },
  },
}

function version(number: string): VersionView {
  return {
    version: number,
    publisher: 'Pingora Panel',
    description: 'The reference plugin, which provides every port.',
    ports: PORTS,
    capabilities: [...PORTS, 'secret-references'],
    protocol_versions: [1],
    compatible: true,
    signed_by: 'reference',
    problems: [],
    config_schema: SCHEMA,
    resources: {
      memory_bytes: 512 * 1024 * 1024,
      cpu_seconds: 0,
      open_files: 256,
      concurrency: 8,
      call_timeout_ms: 10_000,
    },
    executable_sha256: 'ab'.repeat(32),
  }
}

/** Plugins, trusted keys and secrets kept in memory. */
export function pluginHandlers(): AnyHandler[] {
  const now = () => new Date().toISOString()
  let revision = 3
  const plugin: PluginView = {
    name: 'reference',
    state: 'enabled',
    active_version: '1.0.0',
    versions: [version('1.0.0'), version('1.1.0')],
    grants: ['dns01', 'secret-references'],
    settings: { zone: 'example.com', token: 'vault:dns-token', ttl: 120 },
    limits: { memory_bytes: 0, cpu_seconds: 0, open_files: 0, concurrency: 0, call_timeout_ms: 0 },
    effective_limits: version('1.0.0').resources,
    health: {
      status: 'serving',
      version: '1.0.0',
      started_at: new Date(Date.now() - 3_600_000).toISOString(),
      checked_at: now(),
      failures: 0,
      restarts: 0,
    },
    updated_at: now(),
    etag: `"${revision}"`,
  }
  const keys: TrustedKeyView[] = [
    {
      id: 'reference',
      key_id: '9F3C1A7E5B2D4C60',
      public_key: 'RWRgTC1bfhrHn8Q2w7xV0S6dEYq9mPZJx4KcN3bL5a1uT8yWvH2gD0fA',
      comment: 'The reference publisher',
      created_at: new Date(Date.now() - 86_400_000).toISOString(),
    },
  ]
  const secrets: SecretView[] = [{ name: 'dns-token', updated_at: now() }]

  function changed(change: (view: PluginView) => void) {
    change(plugin)
    revision += 1
    plugin.etag = `"${revision}"`
    plugin.updated_at = now()
    if (plugin.health && plugin.active_version) {
      plugin.health = { ...plugin.health, version: plugin.active_version, started_at: now() }
    }
    return HttpResponse.json(plugin, { headers: { etag: plugin.etag } })
  }

  const list = () => ({
    protocol_versions: [1],
    ports: PORTS,
    capabilities: [...PORTS, 'secret-references'],
    limits_enforced: true,
    discovered_at: now(),
    plugins: [plugin],
  })

  return [
    http.get('*/api/v1/plugins', () => HttpResponse.json(list())),
    http.post('*/api/v1/plugins/discover', () => HttpResponse.json(list())),
    http.get('*/api/v1/plugins/reference', () =>
      HttpResponse.json(plugin, { headers: { etag: plugin.etag } }),
    ),
    http.put('*/api/v1/plugins/reference/grants', async ({ request }) => {
      const { capabilities } = (await request.json()) as PluginGrants
      return changed((view) => (view.grants = capabilities))
    }),
    http.put('*/api/v1/plugins/reference/settings', async ({ request }) => {
      const settings = (await request.json()) as Record<string, unknown>
      return changed((view) => (view.settings = settings))
    }),
    http.put('*/api/v1/plugins/reference/limits', async ({ request }) => {
      const limits = (await request.json()) as PluginLimits
      return changed((view) => (view.limits = limits))
    }),
    http.post('*/api/v1/plugins/reference/enable', () =>
      changed((view) => {
        view.state = 'enabled'
        view.health = {
          status: 'serving',
          version: view.active_version ?? '1.0.0',
          started_at: now(),
          failures: 0,
          restarts: 0,
        }
      }),
    ),
    http.post('*/api/v1/plugins/reference/disable', () =>
      changed((view) => {
        view.state = 'disabled'
        view.health = undefined
      }),
    ),
    http.post('*/api/v1/plugins/reference/upgrade', async ({ request }) => {
      const { version: to } = (await request.json()) as { version: string }
      return changed((view) => {
        view.previous_version = view.active_version
        view.active_version = to
      })
    }),
    http.post('*/api/v1/plugins/reference/rollback', () =>
      changed((view) => {
        const to = view.previous_version
        view.previous_version = view.active_version
        view.active_version = to
      }),
    ),
    http.get('*/api/v1/plugin-keys', () => HttpResponse.json(keys)),
    http.post('*/api/v1/plugin-keys', async ({ request }) => {
      const key = (await request.json()) as NewTrustedKey
      const trusted: TrustedKeyView = {
        id: key.id,
        key_id: '0123456789ABCDEF',
        public_key: key.public_key.trim().split('\n').at(-1) ?? '',
        comment: key.comment ?? '',
        created_at: now(),
      }
      keys.push(trusted)
      return HttpResponse.json(trusted, { status: 201 })
    }),
    http.delete('*/api/v1/plugin-keys/:id', ({ params }) => {
      keys.splice(
        keys.findIndex((key) => key.id === params.id),
        1,
      )
      return new HttpResponse(null, { status: 204 })
    }),
    http.get('*/api/v1/plugin-secrets', () => HttpResponse.json(secrets)),
    http.put('*/api/v1/plugin-secrets/:name', ({ params }) => {
      const kept = { name: String(params.name), updated_at: now() }
      const index = secrets.findIndex((secret) => secret.name === kept.name)
      if (index >= 0) {
        secrets[index] = kept
      } else {
        secrets.push(kept)
      }
      return HttpResponse.json(kept)
    }),
    http.delete('*/api/v1/plugin-secrets/:name', ({ params }) => {
      secrets.splice(
        secrets.findIndex((secret) => secret.name === params.name),
        1,
      )
      return new HttpResponse(null, { status: 204 })
    }),
  ]
}
