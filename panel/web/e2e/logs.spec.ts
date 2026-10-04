import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, CSRF_TOKEN, signIn } from './support'

function record(time: string, method: string, path: string, status: number, request_id: string) {
  return {
    time,
    kind: 'access',
    line: JSON.stringify({ 'http.request.method': method, 'url.path': path, status }),
    site: 'shop',
    route: 'app',
    status,
    method,
    path,
    client: '203.0.113.7',
    request_id,
    fields: { http_request_method: method, url_path: path },
  }
}

const page1 = [
  record('2026-10-04T10:00:02.000000007Z', 'GET', '/cart', 502, 'req-2'),
  record('2026-10-04T10:00:01Z', 'GET', '/', 200, 'req-1'),
]

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
  const searches: URLSearchParams[] = []
  await page.route(/\/api\/v1\/logs(\?.*)?$/, (route) => {
    const query = new URL(route.request().url()).searchParams
    searches.push(query)
    const requestId = query.get('request_id')
    const records = page1.filter((item) => !requestId || item.request_id === requestId)
    return route.fulfill({ json: { records, next_until: null } })
  })
  return searches
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('log records are filtered, inspected and followed live', async ({ page }) => {
  const searches = await setUp(page)
  const tails: URL[] = []
  await page.routeWebSocket(/\/api\/v1\/logs\/tail/, (socket) => {
    tails.push(new URL(socket.url()))
    socket.send(
      JSON.stringify({
        records: [record('2026-10-04T10:00:03Z', 'POST', '/api/orders', 201, 'req-3')],
        cursor: '2026-10-04T10:00:03Z',
      }),
    )
  })

  await page.goto('/logs')
  await expect(page.getByRole('heading', { level: 1, name: 'Logs' })).toBeVisible()
  const rows = page.getByRole('row')
  await expect(rows.nth(1)).toContainText('GET /cart')
  await expect(rows.nth(1)).toContainText('502')

  await page.getByLabel('Status').fill('5xx')
  await expect(page).toHaveURL(/status=5xx/)
  await expect.poll(() => searches.at(-1)?.get('status')).toBe('5xx')
  await expect(page.getByRole('link', { name: 'Download' })).toHaveAttribute(
    'href',
    /\/api\/v1\/logs\/download\?status=5xx$/,
  )

  await rows.nth(1).getByRole('button').first().click()
  const sheet = page.getByRole('dialog')
  await expect(sheet.getByText('Line as written')).toBeVisible()
  await expect(sheet.getByText('"url.path":"/cart"')).toBeVisible()
  await sheet.getByRole('button', { name: "Show this request's records" }).click()
  await expect(page).toHaveURL(/request_id=req-2/)
  await expect(page).not.toHaveURL(/status=/)
  await expect(page.getByLabel('Request ID')).toHaveValue('req-2')
  await expect(rows).toHaveCount(2)

  await page.getByRole('button', { name: 'Follow live' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Following live' })).toBeVisible()
  await expect(rows.nth(1)).toContainText('POST /api/orders')
  expect(tails[0]?.searchParams.get('request_id')).toBe('req-2')
  expect(tails[0]?.searchParams.get('after')).toBe('2026-10-04T10:00:02.000000007Z')

  await page.getByRole('button', { name: 'Pause' }).click()
  await expect(page.getByRole('button', { name: 'Follow live' })).toBeVisible()
  await expect(page.getByText('Following live')).toHaveCount(0)
})

test('a tail that ends with an error says so and follows again', async ({ page }) => {
  await setUp(page)
  let attempts = 0
  await page.routeWebSocket(/\/api\/v1\/logs\/tail/, (socket) => {
    attempts += 1
    socket.send(
      JSON.stringify({
        records: [],
        error: { code: 'INVALID_ARGUMENT', message: 'status is neither a code nor a class' },
      }),
    )
  })

  await page.goto('/logs')
  await page.getByRole('button', { name: 'Follow live' }).click()
  const alert = page.getByRole('alert').filter({ hasText: 'Following stopped' })
  await expect(alert).toContainText('status is neither a code nor a class')
  await alert.getByRole('button', { name: 'Follow again' }).click()
  await expect.poll(() => attempts).toBe(2)
})

test('deleting records is confirmed, sent as a command and listed', async ({ page }) => {
  await setUp(page)
  const deletions: { body: unknown; headers: Record<string, string> }[] = []
  await page.route(/\/api\/v1\/logs\/deletions$/, (route) => {
    const request = route.request()
    if (request.method() === 'POST') {
      deletions.push({ body: request.postDataJSON(), headers: request.headers() })
      return route.fulfill({
        status: 202,
        json: {
          site: 'shop',
          since: '1970-01-01T00:00:00Z',
          until: '2026-10-04T10:05:00Z',
          requested_at: '2026-10-04T10:05:00Z',
          state: 'pending',
        },
      })
    }
    return route.fulfill({
      json: {
        deletions: [
          {
            site: null,
            since: '1970-01-01T00:00:00Z',
            until: '2026-09-27T10:00:00Z',
            requested_at: '2026-09-27T10:00:00Z',
            state: 'applied',
          },
        ],
      },
    })
  })

  await page.goto('/logs?site=shop')
  await page.getByRole('button', { name: 'Delete…' }).click()
  const dialog = page.getByRole('alertdialog')
  await expect(dialog.getByLabel('Site')).toHaveValue('shop')
  await dialog.getByRole('button', { name: 'Delete records' }).click()
  await expect(page.getByText('Deletion requested')).toBeVisible()
  expect(deletions).toHaveLength(1)
  expect(deletions[0]?.body).toEqual({ site: 'shop', since: null })
  expect(deletions[0]?.headers['x-csrf-token']).toBe(CSRF_TOKEN)
  expect(deletions[0]?.headers['idempotency-key']).toBeTruthy()

  await page.getByRole('button', { name: 'Deletions' }).click()
  const sheet = page.getByRole('dialog', { name: 'Deletions' })
  await expect(sheet).toContainText('All sites')
  await expect(sheet).toContainText('Applied')
  await expect(sheet).toContainText('From the first record')
})

test('only accounts that may delete logs are offered to', async ({ page }) => {
  await setUp(
    page,
    ALL_PERMISSIONS.filter((permission) => permission !== 'logs.delete'),
  )
  await page.goto('/logs')
  await expect(page.getByRole('button', { name: 'Follow live' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Delete…' })).toHaveCount(0)
})
