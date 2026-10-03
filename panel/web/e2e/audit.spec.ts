import { expect, test, type Page } from '@playwright/test'

function auditEvent(sequence: number, event_type: string, data: Record<string, unknown>) {
  return {
    sequence,
    event_id: `0190b5b6-3f43-7a52-8a56-2f8b7a7d5a1${sequence}`,
    source: '/pingora-panel/config-service',
    event_type,
    event_version: 1,
    subject: 'configuration/draft',
    occurred_at: '2026-10-03T10:00:00.000001Z',
    recorded_at: '2026-10-03T10:00:00.200000Z',
    actor_type: 'user',
    actor_id: 'ops',
    correlation_id: `req-${sequence}`,
    causation_id: `req-${sequence}`,
    data,
    hash: String(sequence).repeat(64).slice(0, 64),
    previous_hash:
      sequence === 1
        ? ''
        : String(sequence - 1)
            .repeat(64)
            .slice(0, 64),
  }
}

const events = [
  auditEvent(3, 'gateway.operation.refused', { operation: 'reloaded', code: 'UNAVAILABLE' }),
  auditEvent(2, 'config.draft.applied', { version: 4, revision: 9, note: 'launch' }),
  auditEvent(1, 'config.draft.changed', {
    version: 4,
    operation: 'sites.create',
    resource: 'sites',
  }),
]

async function setUp(page: Page) {
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
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('audit events are filtered, inspected and verified', async ({ page }) => {
  await setUp(page)
  const queries: URLSearchParams[] = []
  await page.route(/\/api\/v1\/audit-events(\?.*)?$/, (route) => {
    const query = new URL(route.request().url()).searchParams
    queries.push(query)
    const type = query.get('type')
    const correlation = query.get('correlation_id')
    const items = events.filter(
      (event) =>
        (!type || event.event_type === type || event.event_type.startsWith(type)) &&
        (!correlation || event.correlation_id === correlation),
    )
    return route.fulfill({ json: { items, next_before: null } })
  })
  let tampered = false
  await page.route(/\/api\/v1\/audit-events\/verify/, (route) =>
    route.fulfill({
      json: tampered
        ? { intact: false, checked: 1, first_mismatch: 2, head_sequence: 3, head_hash: 'c' }
        : { intact: true, checked: 3, first_mismatch: null, head_sequence: 3, head_hash: 'c' },
    }),
  )

  await page.goto('/audit')
  await expect(page.getByRole('heading', { name: 'Audit log' })).toBeVisible()
  await expect(
    page.getByRole('status').filter({ hasText: 'Refused a gateway operation' }),
  ).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: 'Applied the draft' })).toBeVisible()
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)

  await page.getByRole('combobox', { name: 'Event type' }).click()
  await page.getByRole('option', { name: 'Configuration events' }).click()
  await expect.poll(() => queries.at(-1)?.get('type')).toBe('config.')
  await expect(
    page.getByRole('status').filter({ hasText: 'Refused a gateway operation' }),
  ).toHaveCount(0)
  await expect(page).toHaveURL(/type=config\./)

  await page.getByRole('button', { name: 'Audit record #2' }).click()
  const sheet = page.getByRole('dialog')
  await expect(sheet.getByText('"revision": 9')).toBeVisible()
  await expect(sheet.getByText('ops (user)')).toBeVisible()
  await sheet.getByRole('button', { name: "Show this request's records" }).click()
  await expect.poll(() => queries.at(-1)?.get('correlation_id')).toBe('req-2')
  await expect(page.getByLabel('Correlation ID')).toHaveValue('req-2')

  await page.getByRole('button', { name: 'Verify integrity' }).click()
  await expect(page.getByText('3 records checked; the chain ends at #3.')).toBeVisible()
  tampered = true
  await page.getByRole('button', { name: 'Verify integrity' }).click()
  await expect(page.getByText('Record #2 does not match its hash.')).toBeVisible()
})
