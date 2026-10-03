import { expect, test, type Page, type Request } from '@playwright/test'
import { CSRF_TOKEN, signIn } from './support'

const pending = {
  id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a77',
  state: 'pending',
  draft_version: 6,
  content_hash: 'sha256:cc',
  requested_by: 'alice',
  requested_at: '2026-10-03T09:00:00Z',
  expires_at: '2026-10-04T09:00:00Z',
  note: 'Raise the shop limits',
  risk: 'high',
  policies: [{ id: 'prod', version: 1 }],
  required: 1,
  valid_minutes: 60,
  changes: [{ resource: 'sites/shop', change: 'changed' }],
  approvals: [],
  closed_by: null,
  closed_at: null,
  reason: null,
  revision: null,
}

const mine = {
  ...pending,
  id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a78',
  requested_by: 'root',
  risk: 'low',
  note: null,
}

async function setUp(page: Page) {
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
  await signIn(page)
}

test('approvers decide on requests and see emergency bypasses', async ({ page }) => {
  await setUp(page)
  const decided: Request[] = []
  await page.route(/\/api\/v1\/approvals\?/, (route) =>
    route.fulfill({ json: { items: [pending, mine], next_before: null } }),
  )
  await page.route(`**/api/v1/approvals/${pending.id}/approve`, (route) => {
    decided.push(route.request())
    return route.fulfill({
      json: {
        ...pending,
        state: 'approved',
        approvals: [
          {
            approver: 'root',
            approved_at: '2026-10-03T09:05:00Z',
            valid_until: '2099-01-01T00:00:00Z',
            revoked_at: null,
          },
        ],
      },
    })
  })
  await page.route(/\/api\/v1\/audit-events\?/, (route) =>
    route.fulfill({
      json: {
        items: [
          {
            sequence: 3,
            event_type: 'config.approval.bypassed',
            actor_id: 'root',
            occurred_at: '2026-10-02T22:10:00Z',
            data: { reason: 'checkout is down for everyone', incident: 'INC-7' },
          },
        ],
      },
    }),
  )

  await page.goto('/approvals')
  await expect(page.getByRole('heading', { name: 'Approvals' })).toBeVisible()
  await expect(page.getByText('Recent emergency bypasses')).toBeVisible()
  await expect(page.getByText(/INC-7 · checkout is down for everyone/)).toBeVisible()

  const theirs = page.getByRole('row', { name: /alice/ })
  await expect(theirs.getByText('High risk')).toBeVisible()
  await theirs.getByRole('button', { name: 'Open request' }).click()
  const sheet = page.getByRole('dialog')
  await expect(sheet.getByText('sites/shop')).toBeVisible()
  await expect(sheet.getByRole('button', { name: 'Withdraw request' })).toHaveCount(0)
  await sheet.getByRole('button', { name: 'Approve' }).click()
  await expect(page.getByText('Approved').first()).toBeVisible()
  expect(decided).toHaveLength(1)
  expect(decided[0]!.headers()['x-csrf-token']).toBe(CSRF_TOKEN)
  await expect(sheet.getByText('valid until')).toBeVisible()
  await page.keyboard.press('Escape')

  await page
    .getByRole('row', { name: /root/ })
    .getByRole('button', { name: 'Open request' })
    .click()
  await expect(sheet.getByRole('button', { name: 'Withdraw request' })).toBeVisible()
  await expect(sheet.getByRole('button', { name: 'Approve' })).toHaveCount(0)
})

test('approval policies decide which changes need approval', async ({ page }) => {
  await setUp(page)
  const saved: Request[] = []
  await page.route(/\/api\/v1\/approvals\?/, (route) =>
    route.fulfill({ json: { items: [], next_before: null } }),
  )
  await page.route('**/api/v1/approval-policies', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/approval-policies/prod', (route) => {
    saved.push(route.request())
    return route.fulfill({
      status: 201,
      json: {
        id: 'prod',
        ...route.request().postDataJSON(),
        version: 1,
        created_at: '2026-10-03T00:00:00Z',
        updated_at: '2026-10-03T00:00:00Z',
      },
    })
  })

  await page.goto('/approvals')
  await expect(page.getByText('No approval requests yet.')).toBeVisible()
  await page.getByRole('tab', { name: 'Policies' }).click()
  await expect(page.getByText('No approval policies yet')).toBeVisible()
  await page.getByRole('button', { name: 'New policy' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('prod')
  await sheet.getByLabel('Sites', { exact: true }).click()
  await sheet.getByLabel('Site tags').fill('prod, payments')
  await sheet.getByRole('button', { name: 'Add window' }).click()
  await sheet.getByLabel('Mon', { exact: true }).click()
  await sheet.getByLabel('End', { exact: true }).fill('02:00')
  await sheet.getByLabel('Time zone').fill('Europe/Berlin')
  await sheet.getByLabel('Approvals needed').fill('2')
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Saved the approval policy prod')).toBeVisible()
  expect(saved[0]!.method()).toBe('PUT')
  expect(saved[0]!.postDataJSON()).toEqual({
    description: '',
    resources: ['sites'],
    site_tags: ['prod', 'payments'],
    min_risk: 'low',
    windows: [
      {
        recurrence: expect.stringMatching(
          /^DTSTART;TZID=Europe\/Berlin:\d{8}T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO$/,
        ),
        minutes: 1020,
      },
    ],
    approvals: 2,
    valid_minutes: 60,
    enabled: true,
  })
})

test('applying a covered change waits, and Administrators can bypass it', async ({ page }) => {
  await setUp(page)
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 6, pending: true, applied_version: 5 } }),
  )
  await page.route('**/api/v1/config/schema', (route) =>
    route.fulfill({ json: { language_version: 1, directives: [] } }),
  )
  await page.route('**/api/v1/config/source', (route) =>
    route.fulfill({
      json: {
        language_version: 1,
        version: 6,
        etag: '"draft-6"',
        files: { 'main.conf': 'language_version 1;\n' },
        diagnostics: [],
      },
    }),
  )
  await page.route('**/api/v1/config/check', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  await page.route('**/api/v1/config/plan', (route) =>
    route.fulfill({
      json: {
        resources: [{ resource: 'sites/shop', change: 'changed', diff: '-a\n+b\n' }],
        files: [],
      },
    }),
  )
  const applied: Request[] = []
  await page.route('**/api/v1/config/apply', (route) => {
    applied.push(route.request())
    return route.request().postDataJSON().bypass
      ? route.fulfill({
          json: {
            draft: { version: 6, pending: false, applied_version: 6 },
            revision: 9,
            revision_id: 13,
            content_hash: 'c'.repeat(64),
          },
        })
      : route.fulfill({ status: 202, json: { ...pending, requested_by: 'root' } })
  })

  await page.goto('/config')
  await page.getByRole('button', { name: 'Review changes' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByRole('button', { name: 'Apply v6' }).click()
  await expect(sheet.getByText('Waiting for approval')).toBeVisible()
  await expect(sheet.getByText(/The approval policies prod cover this change/)).toBeVisible()
  const bypass = sheet.getByRole('button', { name: 'Apply without approval' })
  await expect(bypass).toBeDisabled()
  await sheet.getByLabel('Reason (at least 10 characters)').fill('checkout is down for everyone')
  await sheet.getByLabel('Incident').fill('INC-7')
  await bypass.click()
  await expect(page.getByText('Applied as revision #9')).toBeVisible()
  expect(applied[1]!.postDataJSON().bypass).toEqual({
    reason: 'checkout is down for everyone',
    incident: 'INC-7',
  })
})
