import { expect, test, type Request } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

const AUTH = 'if not ngx.var.http_x_key then\n  return ngx.exit(401)\nend\n'

const LIBRARY = {
  disabled: false,
  version: 4,
  revision: null,
  shared_dicts: [{ name: 'hits', capacity_bytes: 1_048_576 }],
  diagnostics: [
    {
      code: 'DSL_LUA',
      severity: 'WARNING',
      message: 'ngx.say is disabled in header_filter_by_lua*',
      source_span: 'main.conf:9.1-20',
    },
  ],
  scripts: [
    {
      id: 'lua/auth.lua',
      file: 'lua/auth.lua',
      line: 1,
      module: 'auth',
      sha256: 'ab'.repeat(32),
      bytes: AUTH.length,
      lines: 3,
      code: AUTH,
      uses: [{ resource: 'lua', label: 'http', phase: 'access' }],
      requires: [],
    },
    {
      id: 'main.conf:12',
      file: 'main.conf',
      line: 12,
      sha256: 'cd'.repeat(32),
      bytes: 15,
      lines: 1,
      code: ' ngx.say("hi") ',
      uses: [{ resource: 'sites/s', label: 'server shop, route hello', phase: 'content' }],
      requires: [],
    },
  ],
}

test('Lua scripts are listed with where they run, read and tested', async ({ page }) => {
  await page.route('**/api/v1/**', (route) =>
    route.fulfill({ status: 404, json: { title: 'Not Found', status: 404 } }),
  )
  await signIn(page, ALL_PERMISSIONS)
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route('**/api/v1/config/lua?*', (route) => route.fulfill({ json: LIBRARY }))
  await page.route('**/api/v1/config/lua', (route) => route.fulfill({ json: LIBRARY }))
  await page.route('**/api/v1/revisions?*', (route) =>
    route.fulfill({ json: { items: [], next_before: null } }),
  )
  await page.route('**/api/v1/config/source', (route) =>
    route.fulfill({
      json: {
        language_version: 1,
        version: 4,
        etag: '"draft-4"',
        files: { 'main.conf': 'language_version 1;\n', 'lua/auth.lua': AUTH },
        diagnostics: [],
      },
    }),
  )
  const tests: Request[] = []
  await page.route('**/api/v1/config/lua/test', (route) => {
    tests.push(route.request())
    return route.fulfill({
      json: {
        draft_version: 4,
        site_id: 'shop',
        route_id: 'hello',
        aborted: false,
        runs: [
          {
            phase: 'content',
            script: 'main.conf:12',
            outcome: 'respond',
            duration_us: 85,
            logs: [{ level: 'notice', message: 'main.conf:12: greeted' }],
          },
        ],
        response: { status: 200, headers: [{ name: 'x-seen', value: '1' }], body: 'hi\n' },
      },
    })
  })

  await page.goto('/lua')
  await expect(page.getByRole('heading', { name: 'Lua scripts' })).toBeVisible()
  const scripts = page.getByRole('navigation', { name: 'Scripts' })
  await expect(scripts.getByRole('button', { name: /lua\/auth\.lua/ })).toHaveAttribute(
    'aria-current',
    'true',
  )
  await expect(page.getByText('ngx.say is disabled in header_filter_by_lua*')).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Lua code: lua/auth.lua' })).toContainText(
    'ngx.exit(401)',
  )
  await expect(page.getByText('Version abababababab')).toBeVisible()

  await scripts.getByRole('button', { name: /main\.conf:12/ }).click()
  await expect(page.getByText('Written at main.conf line 12')).toBeVisible()
  await expect(page.getByText('server shop, route hello')).toBeVisible()

  await page.getByLabel('Host').fill('shop.example')
  await page.getByLabel('Path and query').fill('/hello?who=lua')
  await page.getByLabel('Headers').fill('X-Key: k')
  await page.getByRole('button', { name: 'Run' }).click()
  const results = page.getByRole('region', { name: 'Test results' })
  await expect(results.getByText('Answered')).toBeVisible()
  await expect(results.getByText('main.conf:12: greeted')).toBeVisible()
  await expect(results.getByText('HTTP 200')).toBeVisible()
  expect(tests[0].postDataJSON().request).toMatchObject({
    host: 'shop.example',
    target: '/hello?who=lua',
    headers: [{ name: 'X-Key', value: 'k' }],
  })
})
