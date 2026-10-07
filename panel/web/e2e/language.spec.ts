import { expect, test, type Locator, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

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
const plan = { ...changes, digest: 'd'.repeat(64), draft_version: 5, active_revision: 7 }

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
  await signIn(page)
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
  await page.route('**/api/v1/config/plan', (route) => route.fulfill({ json: plan }))
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
  await page.getByRole('button', { name: 'Import and export' }).click()
  await page.getByRole('menuitem', { name: 'Download IR' }).click()
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
  const confirmation = page.getByRole('alertdialog', { name: 'Apply v5 to the gateway?' })
  await expect(confirmation).toContainText('These changes replace revision #7 on the gateway.')
  await expect(confirmation.getByRole('list', { name: 'What changes' })).toContainText('1 changed')
  await expect(confirmation).toContainText('1 file differs')
  await expect(confirmation).toContainText(`Plan ${'d'.repeat(64)}`)
  expect(applied).toHaveLength(0)
  await confirmation.getByRole('button', { name: 'Apply v5' }).click()
  await expect(page.getByText('Applied as revision #8')).toBeVisible()
  expect(applied[0]!.postDataJSON()).toEqual({
    expected_version: 5,
    expected_plan: 'd'.repeat(64),
    note: 'Rename the shop',
  })
})

test('the editor says when its files load, cannot be read or are empty', async ({ page }) => {
  await mockDraft(page)
  await page.route('**/api/v1/config/check', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  let release!: () => void
  const held = new Promise<void>((resolve) => (release = resolve))
  let reads = 0
  await page.route('**/api/v1/config/source', async (route) => {
    reads += 1
    if (reads <= 2) {
      await held
      return route.fulfill({
        status: 503,
        contentType: 'application/problem+json',
        json: {
          type: 'about:blank',
          title: 'Service Unavailable',
          status: 503,
          code: 'SERVICE_UNAVAILABLE',
          detail: 'the configuration service is starting',
        },
      })
    }
    return route.fulfill({
      json: {
        language_version: 1,
        version: 0,
        etag: '"draft-0"',
        files: { 'main.conf': '' },
        diagnostics: [],
      },
    })
  })

  await page.goto('/config')
  await expect(page.locator('main [data-slot="skeleton"]').first()).toBeVisible()
  release()
  const failure = page
    .getByRole('alert')
    .filter({ hasText: 'the configuration service is starting' })
  await expect(failure).toBeVisible()
  await expectNoHorizontalOverflow(page)
  await failure.getByRole('button', { name: 'Retry' }).click()

  await expect(page.getByText('This file is empty')).toBeVisible()
  await expect(page.getByText(/^Every configuration starts here/)).toBeVisible()
  await expectNoHorizontalOverflow(page)
  await page.getByRole('button', { name: 'Import NGINX' }).click()
  await expect(page.getByRole('dialog', { name: 'Import NGINX' })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog')).toHaveCount(0)

  await page.getByRole('button', { name: 'New file' }).click()
  await page.getByLabel('File path').fill('sites/blog.conf')
  await page.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText(/main\.conf reads them where it includes this file/)).toBeVisible()
  await paste(page.getByRole('textbox', { name: 'Contents of sites/blog.conf' }), SHOP)
  await expect(page.getByText('This file is empty')).toHaveCount(0)
})

test('a plan that changed meanwhile is shown again before it is applied', async ({ page }) => {
  await mockDraft(page)
  await page.route('**/api/v1/config/source', (route) =>
    route.fulfill({
      json: {
        language_version: 1,
        version: 5,
        etag: '"draft-5"',
        files: { 'main.conf': MAIN, 'sites/shop.conf': SHOP },
        diagnostics: [],
      },
    }),
  )
  await page.route('**/api/v1/config/check', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  const plans = [plan, { ...plan, digest: 'e'.repeat(64), active_revision: 8 }]
  let reviewed = 0
  await page.route('**/api/v1/config/plan', (route) =>
    route.fulfill({ json: plans[Math.min(reviewed++, 1)] }),
  )
  const applied: Request[] = []
  await page.route('**/api/v1/config/apply', (route) => {
    applied.push(route.request())
    if (route.request().postDataJSON().expected_plan === 'd'.repeat(64)) {
      return route.fulfill({
        status: 409,
        contentType: 'application/problem+json',
        json: {
          type: 'about:blank',
          title: 'Conflict',
          status: 409,
          code: 'CONFLICT',
          detail: 'the plan changed since it was reviewed',
        },
      })
    }
    return route.fulfill({
      json: {
        draft: { version: 5, pending: false, applied_version: 5 },
        revision: 9,
        revision_id: 13,
        content_hash: 'c'.repeat(64),
      },
    })
  })

  await page.goto('/config')
  await page.getByRole('button', { name: 'Review changes' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByRole('button', { name: 'Apply v5' }).click()
  const confirmation = page.getByRole('alertdialog', { name: 'Apply v5 to the gateway?' })
  await expect(confirmation).toContainText('revision #7')
  await confirmation.getByRole('button', { name: 'Apply v5' }).click()
  await expect(sheet.getByRole('alert')).toContainText('The plan changed since it was shown')
  await expectNoHorizontalOverflow(page)

  await sheet.getByRole('button', { name: 'Apply v5' }).click()
  await expect(confirmation).toContainText('revision #8')
  await confirmation.getByRole('button', { name: 'Apply v5' }).click()
  await expect(page.getByText('Applied as revision #9')).toBeVisible()
  expect(applied.map((request) => request.postDataJSON().expected_plan)).toEqual([
    'd'.repeat(64),
    'e'.repeat(64),
  ])
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
  await page.getByRole('button', { name: 'Import and export' }).click()
  await page.getByRole('menuitem', { name: 'Import NGINX' }).click()
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

test('the whole configuration is exported and imported as one file', async ({ page }) => {
  await mockDraft(page)
  let draft = { version: 4, files: { 'main.conf': MAIN } as Record<string, string> }
  await page.route('**/api/v1/config/source', (route) =>
    route.fulfill({
      json: {
        language_version: 1,
        version: draft.version,
        etag: `"draft-${draft.version}"`,
        files: draft.files,
        diagnostics: [],
      },
    }),
  )
  await page.route('**/api/v1/config/check', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  const imports: Request[] = []
  await page.route('**/api/v1/config/bundle', (route) => {
    if (route.request().method() === 'GET') {
      return route.fulfill({
        json: { format: 'pingora-panel-configuration', language_version: 1, files: draft.files },
      })
    }
    imports.push(route.request())
    draft = { version: draft.version + 1, files: route.request().postDataJSON().files }
    return route.fulfill({
      json: {
        language_version: 1,
        version: draft.version,
        etag: `"draft-${draft.version}"`,
        files: draft.files,
        diagnostics: [],
      },
    })
  })

  await page.goto('/config')
  await expect(page.getByRole('textbox', { name: 'Contents of main.conf' })).toContainText(
    'include sites/*.conf;',
  )
  const download = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Import and export' }).click()
  await page.getByRole('menuitem', { name: 'Export the configuration' }).click()
  expect((await download).suggestedFilename()).toBe('configuration-v4.json')
  await expect(page.getByText('Exported 1 file')).toBeVisible()

  const chooser = page.waitForEvent('filechooser')
  await page.getByRole('button', { name: 'Import and export' }).click()
  await page.getByRole('menuitem', { name: 'Import a configuration' }).click()
  const bundle = {
    format: 'pingora-panel-configuration',
    language_version: 1,
    files: { 'main.conf': MAIN, 'sites/shop.conf': SHOP },
  }
  await (
    await chooser
  ).setFiles({
    name: 'configuration-v9.json',
    mimeType: 'application/json',
    buffer: Buffer.from(JSON.stringify(bundle)),
  })
  await expect(page.getByText('Imported 2 files into the draft')).toBeVisible()
  expect(imports[0]!.postDataJSON()).toEqual(bundle)
  expect(imports[0]!.headers()['if-match']).toBe('"draft-4"')
  const files = page.getByRole('list', { name: 'Files' })
  await expect(files.getByRole('button', { name: 'sites/shop.conf', exact: true })).toBeVisible()

  const refused = page.waitForEvent('filechooser')
  await page.getByRole('button', { name: 'Import and export' }).click()
  await page.getByRole('menuitem', { name: 'Import a configuration' }).click()
  await (
    await refused
  ).setFiles({ name: 'notes.json', mimeType: 'application/json', buffer: Buffer.from('notes') })
  await expect(
    page.getByText('The file is not a configuration exported from a panel'),
  ).toBeVisible()
  expect(imports).toHaveLength(1)
  await expectNoHorizontalOverflow(page)
})

test('the values that apply at the cursor show where they come from', async ({ page }) => {
  await mockDraft(page)
  await page.route('**/api/v1/config/source', (route) =>
    route.fulfill({
      json: {
        language_version: 1,
        version: 4,
        etag: '"draft-4"',
        files: { 'main.conf': MAIN, 'sites/shop.conf': SHOP },
        diagnostics: [],
      },
    }),
  )
  await page.route('**/api/v1/config/check', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  const requests: Request[] = []
  await page.route('**/api/v1/config/explain', (route) => {
    requests.push(route.request())
    return route.fulfill({
      json: {
        block: 'server',
        name: 'shop',
        resource: 'sites/0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a01',
        source_span: 'sites/shop.conf:1.1-4.1',
        settings: [
          { name: 'proxy', value: 'app', source: 'here', source_span: 'sites/shop.conf:3.5-14' },
          {
            name: 'listen',
            value: 'edge',
            source: 'default',
            rule: 'Without it, the server is served on every listener.',
          },
          {
            name: 'tls_profile',
            scope: 'shop.example',
            value: 'edge-cert',
            source: 'inherited',
            from: 'listener secure',
            source_span: 'main.conf:5.9-22',
            rule: "A host uses its domain's tls_profile=, else its server's.",
          },
        ],
      },
    })
  })

  await page.goto('/config')
  await page
    .getByRole('list', { name: 'Files' })
    .getByRole('button', { name: 'sites/shop.conf', exact: true })
    .click()
  await page.getByRole('tab', { name: 'Effective values' }).click()
  await expect(page.getByText('Place the cursor in a server')).toBeVisible()

  const shop = page.getByRole('textbox', { name: 'Contents of sites/shop.conf' })
  await shop.getByText('proxy app;').click()
  const values = page.getByRole('list', { name: 'Effective values' })
  await expect(values.getByText('edge-cert')).toBeVisible()
  expect(requests.at(-1)!.postDataJSON()).toMatchObject({ file: 'sites/shop.conf', line: 3 })
  await expect(page.getByText('Site', { exact: true })).toBeVisible()
  await expect(values.getByRole('img', { name: 'Default' })).toBeVisible()
  await expect(
    values.getByText('Without it, the server is served on every listener.'),
  ).toBeVisible()
  await expect(values.getByText('From listener secure')).toBeVisible()
  await expect(values.getByText('shop.example')).toBeVisible()

  await values.getByRole('button', { name: 'main.conf:5.9-22' }).click()
  await expect(page.getByRole('textbox', { name: 'Contents of main.conf' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
})
