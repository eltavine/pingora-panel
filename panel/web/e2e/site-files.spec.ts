import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

const shop = {
  path: 'shop',
  entries: [
    { name: 'assets', kind: 'directory', size_bytes: 0, modified: null },
    { name: 'index.html', kind: 'file', size_bytes: 13, modified: '2026-10-04T10:00:00Z' },
  ],
}

const top = {
  path: '',
  entries: [
    { name: 'shop', kind: 'directory', size_bytes: 0, modified: null },
    { name: 'logo.png', kind: 'file', size_bytes: 2048, modified: '2026-10-04T10:00:00Z' },
  ],
}

/** What the console asked for, and the bodies it wrote, which WebKit's interception does not expose. */
interface Seen {
  calls: string[]
  bodies: string[]
}

async function setUp(page: Page, permissions = ALL_PERMISSIONS): Promise<Seen> {
  const seen: Seen = { calls: [], bodies: [] }
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
  await page.route(/\/api\/v1\/site-files(\?.*)?$/, (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const path = url.searchParams.get('path') ?? ''
    if (request.method() === 'DELETE') {
      seen.calls.push(`DELETE ${path} recursive=${url.searchParams.get('recursive')}`)
      return route.fulfill({ json: { path, kind: 'directory', removed: 3 } })
    }
    return route.fulfill({ json: path === 'shop' ? shop : top })
  })
  await page.route(/\/api\/v1\/site-files\/content\?/, (route) => {
    const request = route.request()
    const path = new URL(request.url()).searchParams.get('path') ?? ''
    if (request.method() === 'PUT') {
      const condition = request.headers()['if-match'] ?? '-'
      seen.calls.push(`PUT ${path} ${condition}`)
      seen.bodies.push(request.postData() ?? '')
      if (condition === '"stale"') {
        return route.fulfill({
          status: 412,
          contentType: 'application/problem+json',
          json: {
            type: 'about:blank',
            title: 'Precondition failed',
            status: 412,
            code: 'PRECONDITION_FAILED',
            detail: `/${path} changed since it was read`,
          },
        })
      }
      return route.fulfill({
        status: 200,
        headers: { etag: '"t2"' },
        json: { path, size_bytes: 12, sha256: 'ab'.repeat(32), created: false },
      })
    }
    return route.fulfill({
      status: 200,
      headers: { 'content-type': 'application/octet-stream', etag: '"t1"' },
      body: path === 'shop/index.html' ? '<h1>Shop</h1>' : 'PNG',
    })
  })
  await page.route(/\/api\/v1\/site-files\/directories\?/, (route) => {
    seen.calls.push(`POST ${new URL(route.request().url()).searchParams.get('path')}`)
    return route.fulfill({ status: 204, body: '' })
  })
  return seen
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('site files are browsed and edited on the entity tag they were read with', async ({
  page,
  browserName,
}) => {
  const seen = await setUp(page)
  await page.goto('/site-files')
  await expect(page.getByRole('heading', { level: 1, name: 'Site files' })).toBeVisible()
  await page.getByRole('button', { name: 'shop', exact: true }).click()
  await expect(page).toHaveURL(/path=shop/)
  const index = page.getByRole('row').filter({ hasText: 'index.html' })
  await expect(index).toContainText('13 B')
  await index.getByRole('button', { name: 'Edit index.html' }).click()
  const sheet = page.getByRole('dialog', { name: 'shop/index.html' })
  const content = sheet.getByRole('textbox', { name: 'Content of shop/index.html' })
  await expect(content).toHaveValue('<h1>Shop</h1>')
  await content.fill('<h1>New</h1>')
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved shop/index.html')).toBeVisible()
  expect(seen.calls).toEqual(['PUT shop/index.html "t1"'])
  if (browserName !== 'webkit') {
    expect(seen.bodies).toEqual(['<h1>New</h1>'])
  }
})

test('a file changed meanwhile is not overwritten', async ({ page }) => {
  const seen = await setUp(page)
  await page.route(/\/api\/v1\/site-files\/content\?path=shop%2Findex\.html$/, (route) =>
    route.request().method() === 'GET'
      ? route.fulfill({
          headers: { 'content-type': 'application/octet-stream', etag: '"stale"' },
          body: '<h1>Shop</h1>',
        })
      : route.fallback(),
  )
  await page.goto('/site-files?path=shop')
  await page
    .getByRole('row')
    .filter({ hasText: 'index.html' })
    .getByRole('button', { name: 'Edit index.html' })
    .click()
  const sheet = page.getByRole('dialog', { name: 'shop/index.html' })
  await sheet.getByRole('textbox', { name: 'Content of shop/index.html' }).fill('<h1>Mine</h1>')
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(sheet.getByRole('alert')).toContainText('It changed since you opened it')
  await expect(sheet.getByRole('button', { name: 'Reload' })).toBeVisible()
  expect(seen.calls).toEqual(['PUT shop/index.html "stale"'])
})

test('files are uploaded, folders created and directories removed after confirming', async ({
  page,
  browserName,
}) => {
  const seen = await setUp(page)
  await page.goto('/site-files')
  await page.locator('input[type=file]').setInputFiles({
    name: 'robots.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from('User-agent: *'),
  })
  await expect(page.getByText('Uploaded 1 file')).toBeVisible()

  await page.getByRole('button', { name: 'New folder' }).click()
  await page.getByRole('textbox', { name: 'Folder name' }).fill('blog')
  await page.getByRole('button', { name: 'Add' }).click()
  await expect(page.getByText('Created blog')).toBeVisible()

  await page
    .getByRole('row')
    .filter({ hasText: 'shop' })
    .getByRole('button', { name: 'Remove shop' })
    .click()
  const dialog = page.getByRole('alertdialog', { name: 'Remove shop?' })
  await dialog.getByRole('checkbox', { name: 'With everything in it' }).check()
  await dialog.getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Removed shop')).toBeVisible()
  expect(seen.calls).toEqual(['PUT robots.txt -', 'POST blog', 'DELETE shop recursive=true'])
  if (browserName !== 'webkit') {
    expect(seen.bodies).toEqual(['User-agent: *'])
  }
})

test('viewers browse and read files without changing them', async ({ page }) => {
  await setUp(
    page,
    ALL_PERMISSIONS.filter((permission) => permission !== 'files.write'),
  )
  await page.goto('/site-files?path=shop')
  const index = page.getByRole('row').filter({ hasText: 'index.html' })
  await expect(index.getByRole('button', { name: 'Download index.html' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Upload' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'New folder' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: /^Remove / })).toHaveCount(0)
  await index.getByRole('button', { name: 'View index.html' }).click()
  const sheet = page.getByRole('dialog', { name: 'shop/index.html' })
  await expect(sheet.getByRole('textbox')).toHaveAttribute('readonly', '')
  await expect(sheet.getByRole('button', { name: 'Save' })).toHaveCount(0)
})
