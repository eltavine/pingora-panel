import { expect, test, type Page, type Request } from '@playwright/test'

const ACTIVE_HASH = 'a'.repeat(64)
const NEXT_HASH = 'b'.repeat(64)

const readyStatus = {
  ready: true,
  active_revision_id: 6,
  active_hash: ACTIVE_HASH,
  prepared_count: 0,
  adapter_version: 'pingora-v1',
  schema_version: 'v1',
  message: null,
}

async function useEnglish(page: Page) {
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
}

async function mockStatus(page: Page, body: object = readyStatus) {
  await page.route('**/api/v1/gateway/status', (route) => route.fulfill({ json: body }))
}

async function expectNoHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
}

test.beforeEach(async ({ page }) => {
  await useEnglish(page)
  // The preview server sends the production Content Security Policy; any
  // violation means the console would break when served by the API.
  await page.addInitScript(() => {
    const violations: string[] = []
    Object.assign(window, { __cspViolations: violations })
    document.addEventListener('securitypolicyviolation', (event) =>
      violations.push(`${event.violatedDirective} ${event.blockedURI}`),
    )
  })
})

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('the overview reports readiness and the active configuration', async ({ page }) => {
  await mockStatus(page)
  await page.goto('/')

  await expect(page.getByRole('heading', { name: 'Gateway overview' })).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: 'Ready' })).toBeVisible()
  await expect(page.getByText('pingora-v1')).toBeVisible()
  await expect(page.getByTitle(ACTIVE_HASH)).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('an unreachable API is reported with a retry action', async ({ page }) => {
  await page.route('**/api/v1/gateway/status', (route) => route.abort('connectionrefused'))
  await page.goto('/')

  const alert = page.getByRole('alert')
  await expect(alert).toContainText('The management API is unreachable')
  await expect(alert.getByRole('button', { name: 'Retry' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('the color scheme can be switched and persists', async ({ page }) => {
  await mockStatus(page)
  await page.goto('/')

  await page.getByRole('button', { name: 'Appearance' }).click()
  await page.getByRole('menuitem', { name: 'Dark' }).click()
  await expect(page.locator('html')).toHaveClass(/dark/)

  await page.reload()
  await expect(page.locator('html')).toHaveClass(/dark/)
})

test('a snapshot is validated, prepared and activated with compare-and-swap', async ({ page }) => {
  await mockStatus(page)
  const commands: Request[] = []
  await page.route('**/api/v1/gateway/validate', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  await page.route('**/api/v1/gateway/prepare', (route) => {
    commands.push(route.request())
    return route.fulfill({
      json: { prepare_token: 'prepare-7', revision_id: 7, content_hash: NEXT_HASH },
    })
  })
  await page.route('**/api/v1/gateway/activate', (route) => {
    commands.push(route.request())
    return route.fulfill({
      json: { revision_id: 7, content_hash: NEXT_HASH, previous_active_hash: ACTIVE_HASH },
    })
  })

  await page.goto('/publish')
  await page.getByLabel('Snapshot document').fill('{"sites":[],"routes":[]}')
  await page.getByRole('button', { name: 'Validate' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Validation passed' })).toBeVisible()

  await page.getByRole('button', { name: 'Prepare' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Snapshot prepared' })).toBeVisible()
  await expect(page.getByLabel('Expected current hash')).toHaveValue(ACTIVE_HASH)

  await page.getByRole('button', { name: 'Activate' }).click()
  const dialog = page.getByRole('alertdialog')
  await expect(dialog).toContainText('Revision 7 atomically replaces the active configuration.')
  await dialog.getByRole('button', { name: 'Activate' }).click()
  await expect(page.getByText('Revision 7 is active')).toBeVisible()

  const [prepare, activate] = commands
  for (const request of [prepare, activate]) {
    const headers = request!.headers()
    expect(headers['idempotency-key']).toMatch(/^[\x21-\x7e]+$/)
    expect(Date.parse(headers['x-deadline']!)).toBeGreaterThan(Date.now())
  }
  expect(activate!.postDataJSON()).toEqual({
    prepare_token: 'prepare-7',
    expected_active_hash: ACTIVE_HASH,
  })
  await expectNoHorizontalOverflow(page)
})

test('receipts start from an explanatory empty state', async ({ page }) => {
  await page.goto('/receipts')

  await expect(page.getByText('Enter an idempotency key')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Look up' })).toBeDisabled()
  await expectNoHorizontalOverflow(page)
})
