import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

const certificate = {
  id: 'example.com',
  source: 'uploaded',
  subject: 'CN=example.com',
  issuer: 'CN=Example CA',
  serial: '0a1b2c',
  names: ['example.com', '*.example.com', 'shop.example.com', 'api.example.com'],
  not_before: '2026-09-01T00:00:00Z',
  not_after: '2099-12-01T00:00:00Z',
  fingerprint: 'ab'.repeat(32),
  public_key_fingerprint: 'cd'.repeat(32),
  key_algorithm: 'ecdsa_p256',
  key_bits: 256,
  chain_length: 2,
  self_signed: false,
  chain: '-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n',
  version: 3,
  created_at: '2026-09-01T00:00:00Z',
  updated_at: '2026-09-02T00:00:00Z',
  status: 'valid',
  etag: '"3"',
}

const expired = {
  ...certificate,
  id: 'old.example',
  source: 'self_signed',
  names: ['old.example'],
  not_after: '2026-01-01T00:00:00Z',
  status: 'expired',
  version: 1,
  etag: '"1"',
}

async function mockInventory(page: Page, changes: Request[]) {
  await page.route('**/api/v1/certificates', (route) => {
    if (route.request().method() !== 'POST') {
      return route.fulfill({ json: [certificate, expired] })
    }
    changes.push(route.request())
    const body = route.request().postDataJSON() as { id: string; source: string }
    return route.fulfill({
      status: 201,
      headers: { etag: '"1"' },
      json: {
        ...certificate,
        id: body.id,
        source: body.source === 'upload' ? 'uploaded' : 'self_signed',
        version: 1,
        etag: '"1"',
      },
    })
  })
}

test.beforeEach(async ({ page }) => {
  await signIn(page)
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
})

test('certificates show where they stand and are uploaded after a check', async ({ page }) => {
  const changes: Request[] = []
  const inspections: Request[] = []
  await mockInventory(page, changes)
  await page.route('**/api/v1/certificate-inspections', (route) => {
    inspections.push(route.request())
    return route.fulfill({ json: { ...certificate, key_matches: true } })
  })

  await page.goto('/certificates')
  await expect(page.getByRole('heading', { name: 'Certificates' })).toBeVisible()
  const current = page
    .getByRole('row')
    .filter({ has: page.getByText('example.com', { exact: true }) })
  await expect(current.getByText('+1')).toHaveCount(1)
  await expect(current.getByRole('status')).toHaveText(/Valid/)
  const old = page.getByRole('row').filter({ hasText: 'old.example' })
  await expect(old.getByRole('status')).toHaveText(/Expired/)
  await expect(old.getByText('Self-signed')).toBeVisible()

  await page.getByRole('button', { name: 'Upload' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('shop.example')
  await sheet.getByLabel('Certificate chain (PEM)').fill('CHAIN')
  await sheet.getByLabel('Private key (PEM)').fill('KEY')
  await sheet.getByRole('button', { name: 'Check' }).click()
  await expect(sheet.getByText('The key belongs to the certificate')).toBeVisible()
  expect(inspections[0]!.postDataJSON()).toEqual({ chain: 'CHAIN', key: 'KEY' })
  await sheet.getByRole('button', { name: 'Upload' }).click()
  await expect(page.getByText('Uploaded the certificate shop.example')).toBeVisible()
  expect(changes[0]!.postDataJSON()).toEqual({
    source: 'upload',
    id: 'shop.example',
    chain: 'CHAIN',
    key: 'KEY',
  })
  expect(changes[0]!.headers()['idempotency-key']).toBeTruthy()
})

test('certificates are generated, checked against hosts and deleted', async ({ page }) => {
  const changes: Request[] = []
  await mockInventory(page, changes)
  const coverage: Request[] = []
  await page.route('**/api/v1/certificates/example.com/coverage?**', (route) => {
    coverage.push(route.request())
    return route.fulfill({
      json: {
        status: 'valid',
        not_after: certificate.not_after,
        hosts: [
          { host: 'www.example.com', covered: true },
          { host: 'example.org', covered: false },
        ],
      },
    })
  })
  await page.route('**/api/v1/certificates/old.example', (route) => {
    changes.push(route.request())
    return route.fulfill({ status: 204 })
  })

  await page.goto('/certificates')
  await page.getByRole('button', { name: 'Generate' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Names or IP addresses').fill('intranet.example\n*.intranet.example')
  await expect(sheet.getByLabel('Identifier')).toHaveValue('intranet.example')
  await sheet.getByLabel('Days valid').fill('30')
  await sheet.getByRole('button', { name: 'Generate' }).click()
  await expect(page.getByText('Generated the certificate intranet.example')).toBeVisible()
  expect(changes[0]!.postDataJSON()).toEqual({
    source: 'self_signed',
    id: 'intranet.example',
    names: ['intranet.example', '*.intranet.example'],
    days: 30,
  })

  await page
    .getByRole('row')
    .filter({ has: page.getByText('example.com', { exact: true }) })
    .getByRole('button', { name: 'Certificate details' })
    .click()
  const details = page.getByRole('dialog')
  await expect(details.getByText(`AB:${'AB:'.repeat(30)}AB`)).toBeVisible()
  await details.getByRole('textbox', { name: 'Host check' }).fill('www.example.com example.org')
  await details.getByRole('button', { name: 'Check' }).click()
  await expect(details.getByText('Not covered')).toBeVisible()
  expect(new URL(coverage[0]!.url()).searchParams.get('hosts')).toBe('www.example.com,example.org')
  await page.keyboard.press('Escape')

  await page
    .getByRole('row')
    .filter({ hasText: 'old.example' })
    .getByRole('button', { name: 'Delete' })
    .click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Certificate deleted')).toBeVisible()
  expect(changes[1]!.method()).toBe('DELETE')
  expect(changes[1]!.headers()['if-match']).toBe('"1"')
})

test('TLS profiles serve certificates of the inventory', async ({ page }) => {
  await mockInventory(page, [])
  await page.route('**/api/v1/listeners', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/tls-profiles', (route) => route.fulfill({ json: [] }))
  await page.route(/\/api\/v1\/sites\?/, (route) =>
    route.fulfill({ json: { items: [], total: 0, next_cursor: null } }),
  )
  const saved: Request[] = []
  await page.route('**/api/v1/tls-profiles/edge', (route) => {
    saved.push(route.request())
    return route.fulfill({
      json: {
        id: 'edge',
        certificate_id: 'example.com',
        certificate_secret_id: '',
        private_key_secret_id: '',
        min_protocol: 'TLSv1.2',
        alpn: ['h2', 'http/1.1'],
        etag: '"p1"',
      },
    })
  })

  await page.goto('/listeners')
  await page.getByRole('button', { name: 'New TLS profile' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('edge')
  await expect(sheet.getByRole('tab', { name: 'Certificates' })).toHaveAttribute(
    'aria-selected',
    'true',
  )
  await expect(sheet.getByRole('button', { name: 'Save' })).toBeDisabled()
  await sheet.getByRole('combobox', { name: 'Certificate' }).click()
  await page.getByRole('option', { name: /example\.com/ }).click()
  await sheet.getByRole('combobox', { name: 'Maximum TLS version' }).click()
  await page.getByRole('option', { name: 'TLSv1.2' }).click()
  await sheet.getByRole('checkbox', { name: 'TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256' }).click()
  await sheet.getByRole('switch', { name: 'Session resumption' }).click()
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved TLS profile edge')).toBeVisible()
  expect(saved[0]!.postDataJSON()).toEqual({
    id: 'edge',
    certificate_id: 'example.com',
    min_protocol: 'TLSv1.2',
    max_protocol: 'TLSv1.2',
    cipher_suites: ['TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256'],
    session_resumption: false,
    ocsp_stapling: false,
    alpn: ['h2', 'http/1.1'],
  })
})

test('HTTPS listeners are checked as clients see them', async ({ page }) => {
  await page.route('**/api/v1/listeners', (route) =>
    route.fulfill({
      json: [
        {
          id: 'https',
          address: '0.0.0.0:443',
          tls_profile_id: 'edge',
          protocols: { http1: true, http2: true, http3: false },
          reuse_port: false,
          ipv6_only: null,
          default_site_id: null,
          etag: '"l1"',
        },
      ],
    }),
  )
  await page.route('**/api/v1/tls-profiles', (route) => route.fulfill({ json: [] }))
  await page.route(/\/api\/v1\/sites\?/, (route) =>
    route.fulfill({ json: { items: [], total: 0, next_cursor: null } }),
  )
  const checks: Request[] = []
  await page.route('**/api/v1/tls-checks', (route) => {
    checks.push(route.request())
    return route.fulfill({
      json: {
        listener: 'https',
        address: '127.0.0.1:443',
        host: 'shop.example',
        protocol: 'TLSv1.3',
        cipher_suite: 'TLS13_AES_256_GCM_SHA384',
        alpn: 'h2',
        handshake_ms: 4,
        versions: [
          { version: 'TLSv1.2', accepted: false },
          { version: 'TLSv1.3', accepted: true },
        ],
        certificate: { ...certificate, names: ['shop.example'], subject: 'CN=shop.example' },
        certificate_status: 'valid',
        covers_host: true,
        http_status: null,
        strict_transport_security: 'max-age=31536000; includeSubDomains',
      },
    })
  })

  await page.goto('/listeners')
  await page.getByRole('button', { name: 'Check TLS' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByRole('textbox', { name: 'Host to check' }).fill('shop.example')
  await sheet.getByRole('button', { name: 'Check' }).click()
  await expect(sheet.getByText('TLS13_AES_256_GCM_SHA384')).toBeVisible()
  await expect(sheet.getByText('Covers the host')).toBeVisible()
  await expect(sheet.getByText('max-age=31536000; includeSubDomains')).toBeVisible()
  await expect(sheet.getByText('4 ms')).toBeVisible()
  expect(checks[0]!.postDataJSON()).toEqual({ listener: 'https', host: 'shop.example' })
})
