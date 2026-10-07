import { expect, test, type Page, type Request } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

const firing = {
  id: 'shop-errors',
  spec: {
    name: 'Shop errors',
    description: '',
    measure: 'server_error_ratio',
    comparison: 'above',
    threshold: 0.05,
    pending_seconds: 300,
    site: 'shop',
    route: null,
    upstream: null,
    severity: 'critical',
    enabled: true,
    channels: ['ops'],
  },
  version: 3,
  etag: '"3"',
  created_at: '2026-10-04T09:00:00Z',
  updated_at: '2026-10-04T09:00:00Z',
  state: 'firing',
  since: '2026-10-04T09:50:00Z',
  value: 0.083,
  evaluated_at: '2026-10-04T10:00:00Z',
}

const channel = {
  id: 'ops',
  kind: 'webhook',
  target: 'https://hooks.example',
  version: 1,
  etag: '"1"',
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
}

async function setUp(page: Page, permissions = ALL_PERMISSIONS) {
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
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route(/\/api\/v1\/alert-rules$/, (route) => route.fulfill({ json: [firing] }))
  await page.route(/\/api\/v1\/alert-channels$/, (route) =>
    route.request().method() === 'GET' ? route.fulfill({ json: [channel] }) : route.fallback(),
  )
  await page.route(/\/api\/v1\/sites(\?.*)?$/, (route) =>
    route.fulfill({
      json: {
        items: [{ id: 'shop', name: 'Shop', kind: 'reverse_proxy', status: 'running' }],
        total: 1,
        next_cursor: null,
      },
    }),
  )
  await page.route(/\/api\/v1\/upstreams$/, (route) => route.fulfill({ json: [] }))
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('alert rules are listed, created, edited and deleted', async ({ page }) => {
  await setUp(page)
  const changes: Request[] = []
  await page.route(/\/api\/v1\/alert-rules\/[^/]+$/, (route) => {
    changes.push(route.request())
    if (route.request().method() === 'DELETE') {
      return route.fulfill({ status: 204 })
    }
    const body = route.request().postDataJSON()
    return route.fulfill({
      status: route.request().headers()['if-match'] ? 200 : 201,
      json: { ...firing, id: 'new', spec: body, state: 'inactive' },
    })
  })

  await page.goto('/alerts')
  await expect(page.getByRole('heading', { level: 1, name: 'Alerts' })).toBeVisible()
  const row = page.getByRole('row').filter({ hasText: 'Shop errors' })
  await expect(row).toContainText('Firing')
  await expect(row).toContainText('Share of 5xx responses above 5%')
  await expect(row).toContainText('8.3%')

  await page.getByRole('button', { name: 'New rule' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Rule ID').fill('slow')
  await sheet.getByLabel('Name').fill('Slow pages')
  await sheet.getByRole('combobox', { name: 'Measure' }).click()
  await page.getByRole('option', { name: 'P95 latency' }).click()
  await sheet.getByLabel('Threshold').fill('1.5')
  await sheet.getByRole('checkbox').check()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Rule created')).toBeVisible()
  expect(changes[0]?.url()).toMatch(/\/alert-rules\/slow$/)
  expect(changes[0]?.headers()['if-match']).toBeUndefined()
  expect(changes[0]?.postDataJSON()).toMatchObject({
    name: 'Slow pages',
    measure: 'latency_p95',
    comparison: 'above',
    threshold: 1.5,
    pending_seconds: 300,
    site: null,
    severity: 'warning',
    channels: ['ops'],
  })

  await row.getByRole('button', { name: 'Edit Shop errors' }).click()
  await expect(sheet.getByLabel('Threshold')).toHaveValue('5')
  await sheet.getByLabel('Threshold').fill('10')
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved')).toBeVisible()
  expect(changes[1]?.headers()['if-match']).toBe('"3"')
  expect(changes[1]?.postDataJSON()).toMatchObject({ threshold: 0.1, site: 'shop' })

  await row.getByRole('button', { name: 'Delete Shop errors' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Deleted rule Shop errors')).toBeVisible()
  expect(changes[2]?.method()).toBe('DELETE')
  expect(changes[2]?.headers()['if-match']).toBe('"3"')
})

test('channels show their signing secret once and take test notifications', async ({ page }) => {
  await setUp(page)
  let created: Request | undefined
  await page.route(/\/api\/v1\/alert-channels$/, (route) => {
    if (route.request().method() !== 'POST') {
      return route.fallback()
    }
    created = route.request()
    return route.fulfill({
      status: 201,
      json: { channel: { ...channel, id: 'pager' }, secret: 'whsec_c2VjcmV0c2VjcmV0' },
    })
  })
  await page.route(/\/api\/v1\/alert-channels\/ops\/test$/, (route) =>
    route.fulfill({ json: { delivered: true, status: 200 } }),
  )

  await page.goto('/alerts?tab=channels')
  await expect(page.getByRole('cell', { name: 'https://hooks.example' })).toBeVisible()
  await page.getByRole('button', { name: 'New channel' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Channel ID').fill('pager')
  await sheet.getByLabel('Webhook URL').fill('https://hooks.example/T0/secret')
  await sheet.getByRole('button', { name: 'Create' }).click()
  const secret = page.getByRole('alertdialog', { name: 'Signing secret of pager' })
  await expect(secret).toContainText('whsec_c2VjcmV0c2VjcmV0')
  expect(created?.postDataJSON()).toEqual({
    id: 'pager',
    kind: 'webhook',
    url: 'https://hooks.example/T0/secret',
  })
  await secret.getByRole('button', { name: 'I have stored it' }).click()
  await expect(secret).toBeHidden()

  await page.getByRole('button', { name: 'Send a test to ops' }).click()
  await expect(page.getByText('ops took the test notification (200)')).toBeVisible()
})

test('channels can notify through a plugin, which keeps no signing secret', async ({ page }) => {
  await setUp(page)
  let created: Request | undefined
  await page.route(/\/api\/v1\/alert-channels$/, (route) => {
    if (route.request().method() !== 'POST') {
      return route.fallback()
    }
    created = route.request()
    return route.fulfill({
      status: 201,
      json: {
        channel: { ...channel, id: 'chat', kind: 'plugin', target: 'chat/#ops' },
        secret: '',
      },
    })
  })

  await page.goto('/alerts?tab=channels')
  await page.getByRole('button', { name: 'New channel' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Channel ID').fill('chat')
  await sheet.getByRole('radio', { name: 'Plugin' }).click()
  await expect(sheet.getByLabel('Webhook URL')).toHaveCount(0)
  await sheet.getByLabel('Plugin', { exact: true }).fill('chat')
  await sheet.getByLabel('Plugin’s channel').fill('#ops')
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(
    page.getByText('Created channel chat; its plugin delivers notifications'),
  ).toBeVisible()
  await expect(page.getByRole('alertdialog')).toHaveCount(0)
  expect(created?.postDataJSON()).toEqual({
    id: 'chat',
    kind: 'plugin',
    plugin: 'chat',
    plugin_channel: '#ops',
  })
})

test('a rule leads to its notifications', async ({ page }) => {
  await setUp(page)
  const asked: URL[] = []
  await page.route(/\/api\/v1\/alert-notifications(\?.*)?$/, (route) => {
    asked.push(new URL(route.request().url()))
    return route.fulfill({
      json: [
        {
          id: 'n1',
          rule: 'shop-errors',
          channel: 'ops',
          kind: 'firing',
          state: 'abandoned',
          attempts: 7,
          created_at: '2026-10-04T09:55:00Z',
          last_failure: 'the receiver answered 410',
        },
      ],
    })
  })

  await page.goto('/alerts')
  await page.getByRole('button', { name: 'Notifications of Shop errors' }).click()
  await expect(page).toHaveURL(/tab=notifications/)
  await expect(page).toHaveURL(/rule=shop-errors/)
  await expect(page.getByText('Abandoned after 7 attempts')).toBeVisible()
  expect(asked.at(-1)?.searchParams.get('rule')).toBe('shop-errors')
  await page.getByRole('button', { name: 'Every rule' }).click()
  await expect(page).not.toHaveURL(/rule=/)
})

test('accounts that only read alerts are not offered changes', async ({ page }) => {
  await setUp(
    page,
    ALL_PERMISSIONS.filter((permission) => permission !== 'alerts.manage'),
  )
  await page.goto('/alerts')
  await expect(page.getByRole('row').filter({ hasText: 'Shop errors' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'New rule' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Edit Shop errors' })).toHaveCount(0)
})
