import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

function backup(id: string, overrides: Record<string, unknown> = {}) {
  return {
    id,
    contents: ['configuration', 'sites'],
    state: 'completed',
    requested_by: 'ops',
    requested_at: '2026-10-04T10:00:00Z',
    finished_at: '2026-10-04T10:00:05Z',
    size_bytes: 2048,
    sha256: 'ab'.repeat(32),
    files: 12,
    product_version: '0.1.0',
    ...overrides,
  }
}

const failed = backup('b-2', {
  contents: ['certificates'],
  state: 'failed',
  size_bytes: 0,
  failure: { code: 'STORAGE_UNAVAILABLE', message: 'the disk is full' },
})

/** What the console asked for. */
interface Seen {
  created: unknown[]
  restores: unknown[]
  removed: string[]
}

async function setUp(page: Page, permissions = ALL_PERMISSIONS): Promise<Seen> {
  const seen: Seen = { created: [], restores: [], removed: [] }
  await signIn(page, permissions)
  await page.addInitScript(() => {
    window.localStorage.setItem('pingora-panel.locale', 'en')
    const violations: string[] = []
    Object.assign(window, { __cspViolations: violations })
    document.addEventListener('securitypolicyviolation', (event) =>
      violations.push(`${event.violatedDirective} ${event.blockedURI}`),
    )
  })
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 6, pending: false, applied_version: 6 } }),
  )
  await page.route('**/api/v1/backups', (route) => {
    if (route.request().method() === 'POST') {
      seen.created.push(route.request().postDataJSON())
      return route.fulfill({
        status: 202,
        headers: { location: '/api/v1/backups/b-3' },
        json: backup('b-3', { state: 'pending', size_bytes: 0, files: 0 }),
      })
    }
    return route.fulfill({ json: { backups: [backup('b-1'), failed] } })
  })
  await page.route('**/api/v1/backups/b-1/restores', (route) => {
    const body = route.request().postDataJSON()
    seen.restores.push(body)
    return route.fulfill({
      json:
        body.target === 'sites'
          ? { target: 'sites', site_path: body.site_path, files: 3, bytes: 300 }
          : { target: 'configuration', draft_version: 7 },
    })
  })
  await page.route('**/api/v1/backups/b-2', (route) => {
    seen.removed.push(route.request().method())
    return route.fulfill({ status: 204 })
  })
  return seen
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('backups are taken, downloaded, restored and removed', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto('/backups')
  await expect(page.getByRole('heading', { name: 'Backups', level: 1 })).toBeVisible()
  await expect(page.getByText('Configuration').first()).toBeVisible()
  await expect(page.getByText('the disk is full').first()).toBeAttached()

  await page.getByRole('button', { name: 'Take a backup' }).click()
  const dialog = page.getByRole('alertdialog')
  await dialog.getByRole('checkbox', { name: /Sites/ }).click()
  await dialog.getByLabel('Directory below the sites’ directory').fill('/shop/')
  await dialog.getByRole('button', { name: 'Take a backup' }).click()
  await expect(page.getByText('Taking the backup')).toBeVisible()
  expect(seen.created).toEqual([
    { contents: ['configuration', 'certificates', 'sites'], site_path: 'shop' },
  ])

  // Playwright does not route downloads, so the link is checked rather than followed.
  const actions = page.getByRole('button', { name: /^Actions for the backup of/ })
  await actions.first().click()
  const archive = page.getByRole('menuitem', { name: 'Download the archive' })
  await expect(archive).toHaveAttribute('href', '/api/v1/backups/b-1/archive')
  await expect(archive).toHaveAttribute('download', '')
  await page.keyboard.press('Escape')

  await actions.first().click()
  await page.getByRole('menuitem', { name: 'Restore a site directory…' }).click()
  await page
    .getByRole('alertdialog')
    .getByLabel('Directory below the sites’ directory')
    .fill('shop')
  await page.getByRole('alertdialog').getByRole('button', { name: 'Restore' }).click()
  await expect(page.getByText('Restored shop: 3 files')).toBeVisible()

  await actions.first().click()
  await page.getByRole('menuitem', { name: 'Restore the configuration…' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Restore' }).click()
  await expect(page.getByText('Saved as draft v7')).toBeVisible()
  expect(seen.restores).toEqual([
    { target: 'sites', site_path: 'shop' },
    { target: 'configuration' },
  ])

  await actions.nth(1).click()
  await expect(page.getByRole('menuitem', { name: 'Download the archive' })).toHaveCount(0)
  await page.getByRole('menuitem', { name: 'Remove' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Removed the backup')).toBeVisible()
  expect(seen.removed).toEqual(['DELETE'])

  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
})

test('accounts that only read backups cannot change them', async ({ page }) => {
  await setUp(page, ['backups.read'])
  await page.goto('/backups')
  await expect(page.getByText('the disk is full').first()).toBeAttached()
  await expect(page.getByRole('button', { name: 'Take a backup' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: /^Actions for the backup of/ })).toHaveCount(0)
})

test('backups are copied to a plugin target and fetched back', async ({ page }) => {
  await setUp(page)
  const asked: { method: string; path: string; body: unknown }[] = []
  const record = (route: import('@playwright/test').Route) => {
    const request = route.request()
    asked.push({
      method: request.method(),
      path: new URL(request.url()).pathname,
      body: request.postDataJSON(),
    })
  }
  await page.route('**/api/v1/backups/b-1/copies', (route) => {
    record(route)
    return route.fulfill({
      status: 201,
      json: {
        name: 'pingora-panel-backup-b-1.tar.zst',
        size_bytes: 2048,
        sha256: 'ab'.repeat(32),
      },
    })
  })
  await page.route('**/api/v1/backup-targets/s3/archives', (route) =>
    route.fulfill({
      json: [
        {
          name: 'weekly.tar.zst',
          size_bytes: 4096,
          sha256: 'cd'.repeat(32),
          created_at: '2026-10-03T10:00:00Z',
        },
      ],
    }),
  )
  await page.route('**/api/v1/backup-targets/s3/archives/weekly.tar.zst/imports', (route) => {
    record(route)
    return route.fulfill({
      status: 201,
      headers: { location: '/api/v1/backups/b-9' },
      json: backup('b-9', { files: 7 }),
    })
  })
  await page.route('**/api/v1/backup-targets/s3/archives/weekly.tar.zst', (route) => {
    record(route)
    return route.fulfill({ status: 204 })
  })

  await page.goto('/backups')
  await page
    .getByRole('button', { name: /^Actions for the backup of/ })
    .first()
    .click()
  await page.getByRole('menuitem', { name: 'Copy to a plugin target…' }).click()
  const dialog = page.getByRole('alertdialog')
  await dialog.getByLabel('Plugin').fill('s3')
  await dialog.getByRole('button', { name: 'Copy' }).click()
  await expect(page.getByText('Copied to s3 as pingora-panel-backup-b-1.tar.zst')).toBeVisible()

  await page.getByLabel('The plugin whose archives to show').fill('s3')
  await page.getByRole('button', { name: 'Show archives' }).click()
  await expect(page.getByText('weekly.tar.zst')).toBeVisible()
  await page.getByRole('button', { name: 'Actions for weekly.tar.zst' }).click()
  await page.getByRole('menuitem', { name: 'Keep as a backup' }).click()
  await expect(page.getByText('Kept weekly.tar.zst as a backup: 7 files')).toBeVisible()
  await page.getByRole('button', { name: 'Actions for weekly.tar.zst' }).click()
  await page.getByRole('menuitem', { name: 'Remove from the target' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Removed weekly.tar.zst')).toBeVisible()
  expect(asked).toEqual([
    { method: 'POST', path: '/api/v1/backups/b-1/copies', body: { target: 's3' } },
    {
      method: 'POST',
      path: '/api/v1/backup-targets/s3/archives/weekly.tar.zst/imports',
      body: null,
    },
    { method: 'DELETE', path: '/api/v1/backup-targets/s3/archives/weekly.tar.zst', body: null },
  ])
})
