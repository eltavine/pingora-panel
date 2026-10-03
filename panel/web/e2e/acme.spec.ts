import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

const account = {
  id: 'letsencrypt',
  directory: 'https://acme-v02.api.letsencrypt.org/directory',
  ca_bundle: null,
  contact: ['ops@example.com'],
  external_account_key_id: null,
  url: 'https://acme-v02.api.letsencrypt.org/acme/acct/1',
  version: 1,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
  etag: '"1"',
}

const issued = {
  id: 'shop.example',
  account: 'letsencrypt',
  names: ['shop.example', 'www.shop.example'],
  challenge: 'http-01',
  state: 'issued',
  renew_after: '2026-12-01T00:00:00Z',
  renewal_explanation_url: null,
  failures: 0,
  last_error: null,
  version: 3,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-02T00:00:00Z',
  etag: '"3"',
}

const failing = {
  ...issued,
  id: 'dark.example',
  names: ['dark.example'],
  state: 'failing',
  failures: 2,
  last_error: {
    code: 'VALIDATION_FAILED',
    message: 'the CA refused (connection): connection refused',
    at: '2026-10-03T00:00:00Z',
  },
  version: 1,
  etag: '"1"',
}

async function mockAcme(page: Page, accounts: unknown[], changes: Request[]) {
  await page.route('**/api/v1/certificates', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/acme-accounts', (route) => {
    if (route.request().method() !== 'POST') {
      return route.fulfill({ json: accounts })
    }
    changes.push(route.request())
    const body = route.request().postDataJSON() as { id: string; directory: string }
    return route.fulfill({
      status: 201,
      headers: { etag: '"1"' },
      json: { ...account, id: body.id, directory: body.directory },
    })
  })
  await page.route('**/api/v1/acme-accounts/*', (route) => {
    changes.push(route.request())
    return route.fulfill({ status: 204 })
  })
  await page.route('**/api/v1/acme-certificates', (route) => {
    if (route.request().method() !== 'POST') {
      return route.fulfill({ json: [issued, failing] })
    }
    changes.push(route.request())
    const body = route.request().postDataJSON() as { id: string; names: string[] }
    return route.fulfill({
      status: 201,
      headers: { etag: '"1"' },
      json: { ...issued, id: body.id, names: body.names, state: 'pending', etag: '"1"' },
    })
  })
  await page.route('**/api/v1/acme-certificates/**', (route) => {
    changes.push(route.request())
    return route.request().method() === 'DELETE'
      ? route.fulfill({ status: 204 })
      : route.fulfill({ status: 202, json: { ...failing, state: 'failing' } })
  })
}

test.beforeEach(async ({ page }) => {
  await signIn(page)
})

test('ACME accounts are registered with the CA a preset names', async ({ page }) => {
  const changes: Request[] = []
  await mockAcme(page, [], changes)
  await page.goto('/certificates?tab=accounts')
  await expect(page.getByText('No ACME accounts yet')).toBeVisible()
  await page.getByRole('button', { name: 'Register account' }).first().click()

  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Contact email').fill('ops@example.com, dev@example.com')
  const register = sheet.getByRole('button', { name: 'Register account' })
  await expect(register).toBeDisabled()
  await sheet.getByRole('checkbox').click()
  await register.click()

  await expect.poll(() => changes.length).toBe(1)
  expect(changes[0]!.postDataJSON()).toEqual({
    id: 'letsencrypt',
    directory: 'https://acme-v02.api.letsencrypt.org/directory',
    contact: ['ops@example.com', 'dev@example.com'],
    terms_of_service_agreed: true,
  })
})

test('CAs that need it ask for an external account binding', async ({ page }) => {
  const changes: Request[] = []
  await mockAcme(page, [], changes)
  await page.goto('/certificates?tab=accounts')
  await page.getByRole('button', { name: 'Register account' }).first().click()

  const sheet = page.getByRole('dialog')
  await sheet.getByRole('combobox', { name: 'Certificate authority' }).click()
  await page.getByRole('option', { name: 'ZeroSSL' }).click()
  await sheet.getByRole('checkbox').click()
  const register = sheet.getByRole('button', { name: 'Register account' })
  await expect(register).toBeDisabled()
  await sheet.getByLabel('Key ID').fill('kid-1')
  await sheet.getByLabel('MAC key').fill('c2VjcmV0')
  await register.click()

  await expect.poll(() => changes.length).toBe(1)
  expect(changes[0]!.postDataJSON()).toMatchObject({
    id: 'zerossl',
    directory: 'https://acme.zerossl.com/v2/DV90',
    external_account: { key_id: 'kid-1', mac_key: 'c2VjcmV0' },
  })
})

test('automatic certificates are requested, renewed and stopped', async ({ page }) => {
  const changes: Request[] = []
  await mockAcme(page, [account], changes)
  await page.goto('/certificates')
  await page.getByRole('tab', { name: 'Automatic' }).click()
  await expect(page).toHaveURL(/tab=automatic/)

  const failingRow = page.getByRole('row').filter({ hasText: 'dark.example' })
  await expect(failingRow.getByText('Failing')).toBeVisible()
  await expect(failingRow.getByText(/Failed 2 times in a row/)).toBeVisible()
  await expect(
    page.getByRole('row').filter({ hasText: 'www.shop.example' }).getByText('Issued'),
  ).toBeVisible()

  await page.getByRole('button', { name: 'Request certificate' }).click()
  const sheet = page.getByRole('dialog')
  const names = sheet.getByLabel('Names')
  await names.fill('*.api.example')
  await expect(sheet.getByText(/Wildcard names need DNS-01/)).toBeVisible()
  await names.fill('api.example\nwww.api.example')
  await expect(sheet.getByLabel('Identifier')).toHaveValue('api.example')
  await sheet.getByRole('button', { name: 'Request certificate' }).click()
  await expect.poll(() => changes.length).toBe(1)
  expect(changes[0]!.postDataJSON()).toEqual({
    id: 'api.example',
    account: 'letsencrypt',
    names: ['api.example', 'www.api.example'],
    challenge: 'http-01',
  })

  await failingRow.getByRole('button', { name: 'Renew now' }).click()
  await expect.poll(() => changes.length).toBe(2)
  expect(changes[1]!.url()).toMatch(/\/api\/v1\/acme-certificates\/dark\.example\/renewals$/)

  await failingRow.getByRole('button', { name: 'Stop renewing' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Stop renewing' }).click()
  await expect.poll(() => changes.length).toBe(3)
  expect(changes[2]!.method()).toBe('DELETE')
  expect(changes[2]!.headers()['if-match']).toBe('"1"')
})
