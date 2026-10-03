import { expect, test, type Locator, type Page, type Request } from '@playwright/test'

const MAIN = `language_version 1;

http {
    upstream app {
        server 10.0.0.11:8080;
    }

    include sites/*.conf;
}
`
const SHOP = `server shop {
    server_name shop.example;
    proxy app;
}
`
const BROKEN = `language_version 1;

http {
    server s { proxy nowhere; }
}
`

const directives = [
  ['language_version', ['main'], null],
  ['include', ['main', 'http', 'server', 'upstream'], null],
  ['http', ['main'], 'http'],
  ['upstream', ['http'], 'upstream'],
  ['server', ['http'], 'server'],
  ['server_name', ['server'], null],
  ['proxy', ['server', 'route'], null],
].map(([name, contexts, block]) => ({
  name,
  contexts,
  block,
  min_args: 0,
  repeatable: true,
  summary: `${name} summary`,
  syntax: `${name} ...;`,
}))

const changes = {
  resources: [
    {
      resource: 'sites/0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a01',
      change: 'changed',
      diff: '--- a/sites/0b9d\n+++ b/sites/0b9d\n@@ -1,3 +1,3 @@\n server shop {\n-    server_name shop.example;\n+    server_name store.example;\n }\n',
    },
  ],
  files: [
    {
      path: 'sites/shop.conf',
      change: 'changed',
      diff: '--- a/sites/shop.conf\n+++ b/sites/shop.conf\n@@ -1,2 +1,2 @@\n server shop {\n-    server_name shop.example;\n+    server_name store.example;\n',
    },
  ],
}

function revision(id: number, outcome: string, note: string | null = null) {
  return {
    id,
    draft_version: id,
    language_version: 1,
    content_hash: 'a'.repeat(64),
    author: 'web-console',
    note,
    created_at: '2026-10-01T10:00:00Z',
    outcome,
    outcome_at: '2026-10-01T10:00:02Z',
    diagnostics: [],
    snapshot_hash: 'b'.repeat(64),
    gateway_revision: id + 4,
  }
}

async function useEnglish(page: Page) {
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
}

async function expectNoHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
}

/** Replaces the editor's text the way a person pasting it would. */
async function paste(editor: Locator, text: string) {
  await editor.click()
  await editor.press('ControlOrMeta+a')
  await editor.evaluate((element, value) => {
    const data = new DataTransfer()
    data.setData('text/plain', value)
    element.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }),
    )
  }, text)
}

async function mockDraft(page: Page) {
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: true, applied_version: 3 } }),
  )
  await page.route('**/api/v1/config/schema', (route) =>
    route.fulfill({ json: { language_version: 1, directives } }),
  )
}

test.beforeEach(async ({ page }) => {
  await useEnglish(page)
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

test('configuration files are checked as they are edited, saved and applied', async ({ page }) => {
  await mockDraft(page)
  const saved: Request[] = []
  await page.route('**/api/v1/config/source', (route) => {
    if (route.request().method() === 'PUT') {
      saved.push(route.request())
      const files = route.request().postDataJSON().files
      return route.fulfill({
        json: { language_version: 1, version: 5, etag: '"draft-5"', files, diagnostics: [] },
      })
    }
    return route.fulfill({
      json: {
        language_version: 1,
        version: 4,
        etag: '"draft-4"',
        files: { 'main.conf': MAIN, 'sites/shop.conf': SHOP },
        diagnostics: [],
      },
    })
  })
  await page.route('**/api/v1/config/check', (route) => {
    const broken = JSON.stringify(route.request().postDataJSON()).includes('nowhere')
    return route.fulfill({
      json: broken
        ? {
            valid: false,
            diagnostics: [
              {
                code: 'DSL_REFERENCE',
                severity: 'ERROR',
                message: 'no upstream is named "nowhere"',
                source_span: 'main.conf:4.22-28',
              },
            ],
          }
        : { valid: true, diagnostics: [] },
    })
  })
  await page.route('**/api/v1/config/plan', (route) => route.fulfill({ json: changes }))
  await page.route('**/api/v1/config/ast', (route) =>
    route.fulfill({
      json: {
        file: route.request().postDataJSON().file,
        directives: [
          { name: 'language_version', args: ['1'], span: 'main.conf:1.1-19' },
          {
            name: 'http',
            args: [],
            span: 'main.conf:3.1-9.1',
            block: [{ name: 'upstream', args: ['app'], span: 'main.conf:4.5-6.5', block: [] }],
          },
        ],
        diagnostics: [],
      },
    }),
  )
  await page.route('**/api/v1/config/ir', (route) =>
    route.fulfill({ json: { schema_version: 'panel.ir.v1', listeners: [], sites: [] } }),
  )
  await page.route('**/api/v1/config/dry-run', (route) =>
    route.fulfill({
      json: { draft: { version: 5, pending: true, applied_version: 3 }, diagnostics: [] },
    }),
  )
  const applied: Request[] = []
  await page.route('**/api/v1/config/apply', (route) => {
    applied.push(route.request())
    return route.fulfill({
      json: {
        draft: { version: 5, pending: false, applied_version: 5 },
        revision: 8,
        revision_id: 12,
        content_hash: 'c'.repeat(64),
      },
    })
  })

  await page.goto('/config')
  await expect(page.getByRole('heading', { name: 'Configuration files' })).toBeVisible()
  const files = page.getByRole('list', { name: 'Files' })
  await expect(files.getByRole('button', { name: 'sites/shop.conf', exact: true })).toBeVisible()
  const editor = page.getByRole('textbox', { name: 'Contents of main.conf' })
  await expect(editor).toContainText('include sites/*.conf;')
  await expect(page.getByText('No problems found')).toBeVisible()
  await expectNoHorizontalOverflow(page)

  await page.getByRole('tab', { name: 'Outline' }).click()
  await page.getByRole('button', { name: /upstream\s+app/ }).click()
  const download = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Download IR' }).click()
  expect((await download).suggestedFilename()).toBe('config-ir-v4.json')
  await page.getByRole('tab', { name: /Problems/ }).click()

  await editor.click()
  await editor.press('ControlOrMeta+End')
  await page.keyboard.type('ht')
  await expect(page.getByRole('option', { name: /^http/ })).toBeVisible()
  await page.keyboard.press('Escape')
  await page.keyboard.press('Backspace')
  await page.keyboard.press('Backspace')

  await paste(editor, BROKEN)
  await expect(page.getByText('no upstream is named "nowhere"')).toBeVisible()
  await expect(page.getByRole('button', { name: 'main.conf:4.22-28' })).toBeVisible()
  await expect(files.getByRole('img', { name: 'Unsaved' })).toBeVisible()

  await page.getByRole('button', { name: 'Discard changes' }).click()
  await expect(editor).toContainText('include sites/*.conf;')
  await files.getByRole('button', { name: 'sites/shop.conf', exact: true }).click()
  const shop = page.getByRole('textbox', { name: 'Contents of sites/shop.conf' })
  await paste(shop, SHOP.replace('shop.example', 'store.example'))
  await expect(page.getByText('No problems found')).toBeVisible()
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(page.getByText('Saved as draft v5')).toBeVisible()
  expect(saved[0]!.headers()['if-match']).toBe('"draft-4"')
  expect(saved[0]!.postDataJSON().files['sites/shop.conf']).toContain('store.example')

  await page.getByRole('button', { name: 'Review changes' }).click()
  const sheet = page.getByRole('dialog')
  await expect(sheet.getByText('sites/shop.conf').first()).toBeVisible()
  await expect(
    sheet.getByText('+    server_name store.example;').filter({ visible: true }).first(),
  ).toBeVisible()
  await sheet.getByRole('button', { name: 'Dry run' }).click()
  await expect(page.getByText('Dry run passed: draft v5 can be applied')).toBeVisible()
  await sheet.getByLabel('Revision note').fill('Rename the shop')
  await sheet.getByRole('button', { name: 'Apply v5' }).click()
  await expect(page.getByText('Applied as revision #8')).toBeVisible()
  expect(applied[0]!.postDataJSON()).toEqual({ expected_version: 5, note: 'Rename the shop' })
})

test('revisions are compared, annotated and rolled back', async ({ page }) => {
  await mockDraft(page)
  await page.route(/\/api\/v1\/revisions(\?.*)?$/, (route) =>
    route.fulfill({
      json: { items: [revision(8, 'active', 'Rename the shop'), revision(7, 'superseded')] },
    }),
  )
  const comparisons: string[] = []
  await page.route(/\/api\/v1\/revisions\/7\/diff/, (route) => {
    comparisons.push(new URL(route.request().url()).searchParams.get('against') ?? '')
    return route.fulfill({ json: changes })
  })
  await page.route('**/api/v1/revisions/7', (route) =>
    route.fulfill({
      json: {
        revision: revision(7, 'superseded'),
        files: { 'main.conf': MAIN, 'sites/shop.conf': SHOP },
      },
    }),
  )
  await page.route('**/api/v1/revisions/9', (route) =>
    route.fulfill({
      json: { revision: revision(9, 'active', 'Bad deploy'), files: { 'main.conf': MAIN } },
    }),
  )
  await page.route(/\/api\/v1\/revisions\/9\/diff/, (route) =>
    route.fulfill({ json: { resources: [], files: [] } }),
  )
  await page.route('**/api/v1/revisions/7/note', (route) =>
    route.fulfill({ json: revision(7, 'superseded', route.request().postDataJSON().note) }),
  )
  const restored: Request[] = []
  await page.route('**/api/v1/revisions/7/restore', (route) => {
    restored.push(route.request())
    return route.fulfill({
      json: {
        language_version: 1,
        version: 6,
        etag: '"draft-6"',
        files: { 'main.conf': MAIN },
        diagnostics: [],
      },
    })
  })
  const applied: Request[] = []
  await page.route('**/api/v1/config/apply', (route) => {
    applied.push(route.request())
    return route.fulfill({
      json: {
        draft: { version: 6, pending: false, applied_version: 6 },
        revision: 9,
        revision_id: 13,
        content_hash: 'c'.repeat(64),
      },
    })
  })

  await page.goto('/revisions')
  await expect(page.getByRole('heading', { name: 'Revisions' })).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: 'Active' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
  await page.getByRole('link', { name: '#7' }).click()

  await expect(page.getByRole('heading', { name: 'Revision #7' })).toBeVisible()
  await expect(
    page.getByText('+    server_name store.example;').filter({ visible: true }).first(),
  ).toBeVisible()
  await page.getByRole('combobox', { name: 'Compare with' }).click()
  await page.getByRole('option', { name: 'Active revision' }).click()
  await expect.poll(() => comparisons).toContain('active')

  await page.getByRole('button', { name: 'Edit note' }).click()
  await page.getByRole('textbox', { name: 'Note' }).fill('Before the rename')
  await page.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Note saved')).toBeVisible()
  await expect(page.getByText('Before the rename')).toBeVisible()

  await page.getByRole('tab', { name: 'Files' }).click()
  await expect(page.getByRole('textbox', { name: 'Contents of main.conf' })).toContainText(
    'language_version 1;',
  )

  await page.getByRole('button', { name: 'Roll back' }).click()
  const dialog = page.getByRole('alertdialog')
  await dialog.getByLabel('Reason').fill('Bad deploy')
  await dialog.getByRole('button', { name: 'Roll back' }).click()
  await expect(page.getByText('Rolled back; revision #9 is active')).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Revision #9' })).toBeVisible()
  expect(restored).toHaveLength(1)
  expect(applied[0]!.postDataJSON()).toEqual({ expected_version: 6, note: 'Bad deploy' })
})

test('NGINX configuration is converted and loaded into the editor', async ({ page }) => {
  await mockDraft(page)
  await page.route('**/api/v1/config/source', (route) =>
    route.fulfill({
      json: {
        language_version: 1,
        version: 4,
        etag: '"draft-4"',
        files: { 'main.conf': MAIN },
        diagnostics: [],
      },
    }),
  )
  await page.route('**/api/v1/config/check', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  const converted =
    'language_version 1;\n\nhttp {\n    server imported {\n        respond 204;\n    }\n}\n'
  const requests: Request[] = []
  await page.route('**/api/v1/config/import/nginx', (route) => {
    requests.push(route.request())
    return route.fulfill({
      json: {
        files: { 'main.conf': converted },
        report: [
          {
            code: 'NGINX_UNSUPPORTED',
            severity: 'WARNING',
            message: "'gzip' is not supported",
            source_span: 'nginx.conf:2.5-12',
          },
        ],
        valid: true,
        diagnostics: [],
      },
    })
  })

  await page.goto('/config')
  await page.getByRole('button', { name: 'Import NGINX' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('NGINX configuration').fill('http {\n    gzip on;\n}\n')
  await sheet.getByRole('button', { name: 'Convert' }).click()
  await expect(sheet.getByText("'gzip' is not supported")).toBeVisible()
  await expect(sheet.getByText('Not fully carried over (1)')).toBeVisible()
  expect(requests[0]!.postDataJSON()).toEqual({
    files: { 'nginx.conf': 'http {\n    gzip on;\n}\n' },
    entry: 'nginx.conf',
  })
  await sheet.getByRole('button', { name: 'Load into the editor' }).click()
  await expect(page.getByText('The converted configuration is in the editor')).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Contents of main.conf' })).toContainText(
    'server imported',
  )
  await expect(
    page.getByRole('list', { name: 'Files' }).getByRole('img', { name: 'Unsaved' }),
  ).toBeVisible()
})
