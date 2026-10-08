import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

const versions = {
  observed_at: '2026-10-08T12:00:00Z',
  release: '0.9.0',
  commit: '0123abc',
  api: 'v1',
  language: 1,
  ir_schema: '1.0.0',
  modules: [
    {
      service: 'config-service',
      instance_id: '0196f1c8-7d1e-7c3a-9f0e-1b2c3d4e5f60',
      build_version: '0.3.0',
      schema_version: '20',
      started_at: '2026-10-08T11:00:00Z',
      protocols: [{ name: 'pingora.panel.config.v1', min_revision: 1, max_revision: 2 }],
      capabilities: [],
    },
  ],
  gateway: { gateway: '0.9.0', engine: '0.9.0', adapter: 'pingora' },
  deployment: {
    changed_at: '2026-10-08T11:30:00Z',
    action: 'upgrade',
    engine: 'podman',
    project: 'pingora-panel',
    previous: '0.8.0',
    images: [
      {
        service: 'control',
        image: 'ghcr.io/example/pingora-panel:0.9.0',
        digest: `sha256:${'ab'.repeat(32)}`,
      },
    ],
  },
  problems: ['the host agent cannot be read: it does not answer'],
}

const readiness = {
  observed_at: '2026-10-08T12:00:00Z',
  ready: false,
  checks: [
    { name: 'modules', state: 'pass', detail: 'every module is healthy' },
    {
      name: 'gateway',
      state: 'fail',
      detail: 'snapshots prepared and not activated: 1; activate or abort them first',
    },
    {
      name: 'backup',
      state: 'warn',
      detail: 'no backup was taken yet; the upgrade takes one first',
    },
  ],
}

const bundle = {
  generated_at: '2026-10-08T12:00:00Z',
  versions,
  readiness,
  backups: [],
  alerts: [],
  plugins: [],
  audit: [],
  withheld: [],
  problems: [],
}

async function setUp(page: Page, permissions = ALL_PERMISSIONS) {
  await signIn(page, permissions)
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route('**/api/v1/system/versions', (route) => route.fulfill({ json: versions }))
  await page.route('**/api/v1/system/preflight', (route) => route.fulfill({ json: readiness }))
  await page.route('**/api/v1/system/diagnostics', (route) => route.fulfill({ json: bundle }))
}

test('the system page shows what runs, what holds an upgrade back and saves the bundle', async ({
  page,
}) => {
  await setUp(page)
  await page.goto('/system')
  await expect(page.getByRole('heading', { level: 1, name: 'System' })).toBeVisible()
  await expect(page.getByText('0.9.0 (0123abc)')).toBeVisible()
  await expect(page.getByText(/Upgraded on Podman, .* · from 0\.8\.0/)).toBeVisible()
  await expect(page.getByRole('row').filter({ hasText: 'config-service' })).toContainText(
    'pingora.panel.config.v1 1–2',
  )
  await expect(page.getByRole('row').filter({ hasText: 'control' })).toContainText(
    'sha256:abababababab',
  )
  await expect(page.getByText('the host agent cannot be read')).toBeVisible()

  await expect(page.getByText('An upgrade cannot start yet')).toBeVisible()
  const gateway = page.getByRole('listitem').filter({ hasText: 'activate or abort them first' })
  await expect(gateway.getByRole('status')).toHaveText('Failed')
  const backup = page.getByRole('listitem').filter({ hasText: 'no backup was taken yet' })
  await expect(backup.getByRole('status')).toHaveText('Attention')

  const download = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Download the bundle' }).click()
  expect((await download).suggestedFilename()).toBe(
    'pingora-panel-diagnostics-20261008T120000Z.json',
  )
  await expect(
    page.getByText('Downloaded pingora-panel-diagnostics-20261008T120000Z.json'),
  ).toBeVisible()
})

test('the bundle is offered only to accounts that diagnose', async ({ page }) => {
  await setUp(
    page,
    ALL_PERMISSIONS.filter((permission) => permission !== 'platform.diagnose'),
  )
  await page.goto('/system')
  await expect(page.getByText('needs the platform.diagnose permission')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Download the bundle' })).toHaveCount(0)
})
