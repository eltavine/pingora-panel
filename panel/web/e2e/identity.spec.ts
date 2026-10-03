import { expect, test, type Page, type Request } from '@playwright/test'
import { CSRF_TOKEN, currentSession, signIn } from './support'

const PASSWORD = 'glacier violin tapestry orbit'

const viewer = {
  id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a60',
  username: 'watcher',
  display_name: 'Night watch',
  disabled: false,
  locked: true,
  roles: ['viewer'],
  created_at: '2026-10-02T08:00:00Z',
  updated_at: '2026-10-02T08:00:00Z',
  last_login_at: null,
  password_changed_at: '2026-10-02T08:00:00Z',
}

const roles = [
  {
    id: 'administrator',
    name: 'Administrator',
    description: 'Everything, accounts and roles included.',
    permissions: [],
    built_in: true,
  },
  {
    id: 'viewer',
    name: 'Viewer',
    description: 'Reads configuration and the gateway’s state.',
    permissions: ['config.read'],
    built_in: true,
  },
]

function problem(status: number, code: string, detail: string, field_errors: object[] = []) {
  return {
    status,
    contentType: 'application/problem+json',
    json: {
      type: `urn:pingora-panel:error:${code}`,
      title: 'Error',
      status,
      code,
      detail,
      retryable: false,
      field_errors,
    },
  }
}

async function setUp(page: Page) {
  await page.addInitScript(() => {
    window.localStorage.setItem('pingora-panel.locale', 'en')
    const violations: string[] = []
    Object.assign(window, { __cspViolations: violations })
    document.addEventListener('securitypolicyviolation', (event) =>
      violations.push(`${event.violatedDirective} ${event.blockedURI}`),
    )
  })
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('visitors log in and return to the page they asked for', async ({ page }) => {
  await setUp(page)
  let signedIn = false
  const logins: Request[] = []
  await page.route('**/api/v1/session', (route) => {
    if (route.request().method() === 'POST') {
      logins.push(route.request())
      if (route.request().postDataJSON().password !== PASSWORD) {
        return route.fulfill(problem(401, 'UNAUTHENTICATED', 'invalid username or password'))
      }
      signedIn = true
      return route.fulfill({ status: 201, json: { ...currentSession(), secret: null } })
    }
    return signedIn
      ? route.fulfill({ json: currentSession() })
      : route.fulfill(problem(401, 'UNAUTHENTICATED', 'log in or present an API token'))
  })
  await page.route('**/api/v1/setup', (route) => route.fulfill({ json: { required: false } }))
  await page.route('**/api/v1/account/sessions', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/account/tokens', (route) => route.fulfill({ json: [] }))

  await page.goto('/account')
  await expect(page).toHaveURL(/\/login\?next=\/account$/)
  await expect(page.getByRole('heading', { name: 'Log in to Pingora Panel' })).toBeVisible()
  await page.getByLabel('Username').fill('root')
  await page.getByLabel('Password', { exact: true }).fill('not the password')
  await page.getByRole('button', { name: 'Show password' }).click()
  await expect(page.getByLabel('Password', { exact: true })).toHaveAttribute('type', 'text')
  await page.getByRole('button', { name: 'Log in' }).click()
  await expect(page.getByRole('alert')).toContainText('Wrong username or password')
  await expect(page.getByLabel('Password', { exact: true })).toHaveValue('')

  await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
  await page.getByRole('button', { name: 'Log in' }).click()
  await expect(page).toHaveURL(/\/account$/)
  await expect(page.getByRole('heading', { name: 'Account settings' })).toBeVisible()
  expect(logins[1]!.postDataJSON()).toEqual({
    username: 'root',
    password: PASSWORD,
    transport: 'cookie',
  })
})

test('the first administrator is created with the bootstrap token', async ({ page }) => {
  await setUp(page)
  let created = false
  const setups: Request[] = []
  await page.route('**/api/v1/setup', (route) => {
    if (route.request().method() === 'GET') {
      return route.fulfill({ json: { required: !created } })
    }
    setups.push(route.request())
    if (route.request().postDataJSON().password.length < 15) {
      return route.fulfill(
        problem(422, 'VALIDATION_FAILED', 'the password must have at least 15 characters', [
          {
            code: 'VALIDATION_FAILED',
            severity: 'ERROR',
            message: 'the password must have at least 15 characters',
            resource_id: 'password',
            help: 'a few unrelated words make a long password that is easy to remember',
          },
        ]),
      )
    }
    created = true
    return route.fulfill({ status: 201, json: currentSession().account })
  })
  await page.route('**/api/v1/session', (route) =>
    route.request().method() === 'POST'
      ? route.fulfill({ status: 201, json: { ...currentSession(), secret: null } })
      : created
        ? route.fulfill({ json: currentSession() })
        : route.fulfill(problem(401, 'UNAUTHENTICATED', 'log in')),
  )

  await page.goto('/sites')
  await expect(page).toHaveURL(/\/setup$/)
  await page.getByLabel('Bootstrap token').fill('  the-bootstrap-token  ')
  await page.getByLabel('Username').fill('root')
  await page.getByLabel('Password', { exact: true }).fill('too short')
  await page.getByLabel('Confirm password').fill('too shorter')
  await expect(page.getByText('The passwords differ')).toBeVisible()
  await page.getByLabel('Confirm password').fill('too short')
  await page.getByRole('button', { name: 'Create and log in' }).click()
  await expect(page.getByText('the password must have at least 15 characters')).toBeVisible()
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
  await page.getByLabel('Confirm password').fill(PASSWORD)
  await page.getByRole('button', { name: 'Create and log in' }).click()
  await expect(page).toHaveURL(/\/$/)
  expect(setups[1]!.postDataJSON()).toEqual({
    token: 'the-bootstrap-token',
    username: 'root',
    password: PASSWORD,
    display_name: null,
  })
})

test('accounts keep their own tokens and only see what they may use', async ({ page }) => {
  await setUp(page)
  await signIn(page, ['config.read', 'gateway.read'])
  const created: Request[] = []
  await page.route('**/api/v1/account/sessions', (route) =>
    route.fulfill({
      json: [
        currentSession().session,
        { ...currentSession().session, id: 'other', transport: 'bearer', current: false },
      ],
    }),
  )
  await page.route('**/api/v1/account/tokens', (route) => {
    if (route.request().method() === 'POST') {
      created.push(route.request())
      return route.fulfill({
        status: 201,
        json: {
          token: {
            id: 't1',
            name: 'ci',
            permissions: ['config.read'],
            created_at: '2026-10-03T08:00:00Z',
            expires_at: '2026-11-02T08:00:00Z',
          },
          secret: 'ppat_shown_once',
        },
      })
    }
    return route.fulfill({ json: [] })
  })

  await page.goto('/account')
  await expect(page.getByRole('heading', { name: 'Account settings' })).toBeVisible()
  await expect(page.getByText('Command line')).toBeVisible()
  // Administration is neither listed nor reachable.
  await expect(page.getByRole('link', { name: 'Accounts' })).toHaveCount(0)
  await page.goto('/accounts')
  await expect(page).toHaveURL(/\/account$/)

  await page.getByRole('button', { name: 'Create token' }).first().click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Token name').fill('ci')
  await sheet.getByLabel('gateway.read').click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(sheet.getByText('ppat_shown_once')).toBeVisible()
  expect(created[0]!.headers()['x-csrf-token']).toBe(CSRF_TOKEN)
  expect(created[0]!.postDataJSON()).toEqual({
    name: 'ci',
    permissions: ['config.read'],
    expires_in_days: 90,
  })
  await sheet.getByRole('button', { name: 'Done' }).click()
})

test('administrators manage accounts and log out', async ({ page }) => {
  await setUp(page)
  await signIn(page)
  const changes: Request[] = []
  await page.route('**/api/v1/accounts', (route) =>
    route.request().method() === 'POST'
      ? (changes.push(route.request()),
        route.fulfill({ status: 201, json: { ...viewer, username: 'ops', locked: false } }))
      : route.fulfill({ json: [currentSession().account, viewer] }),
  )
  await page.route(`**/api/v1/accounts/${viewer.id}`, (route) => {
    changes.push(route.request())
    return route.fulfill({ json: { ...viewer, locked: false } })
  })
  await page.route('**/api/v1/roles', (route) => route.fulfill({ json: roles }))

  await page.goto('/accounts')
  await expect(page.getByRole('heading', { name: 'Accounts' })).toBeVisible()
  const row = page.getByRole('row', { name: /watcher/ })
  await expect(row.getByText('Password locked')).toBeVisible()
  await row.getByRole('button', { name: 'Actions' }).click()
  await page.getByRole('menuitem', { name: 'Unlock password' }).click()
  await expect(page.getByText('Password unlocked')).toBeVisible()
  expect(changes[0]!.method()).toBe('PATCH')
  expect(changes[0]!.postDataJSON()).toEqual({ unlock: true })
  expect(changes[0]!.headers()['x-csrf-token']).toBe(CSRF_TOKEN)

  await page.getByRole('button', { name: 'New account' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Username').fill('ops')
  await sheet.getByLabel('Administrator').click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Created the account ops')).toBeVisible()
  expect(changes[1]!.postDataJSON()).toEqual({
    username: 'ops',
    display_name: null,
    password: null,
    roles: ['viewer', 'administrator'],
  })

  let ended = false
  await page.route('**/api/v1/session', (route) => {
    if (route.request().method() === 'DELETE') {
      ended = true
      return route.fulfill({ status: 204 })
    }
    return route.fulfill({ json: currentSession() })
  })
  await page.getByRole('button', { name: 'Logged in as root' }).click()
  await page.getByRole('menuitem', { name: 'Log out' }).click()
  await expect(page).toHaveURL(/\/login$/)
  expect(ended).toBe(true)
})

test('custom roles are created, edited and deleted', async ({ page }) => {
  await setUp(page)
  await signIn(page)
  const changes: Request[] = []
  const custom = {
    id: 'deployer',
    name: 'Deployer',
    description: 'Applies configuration.',
    permissions: ['config.read', 'config.apply'],
    built_in: false,
  }
  await page.route('**/api/v1/roles', (route) => {
    if (route.request().method() === 'POST') {
      changes.push(route.request())
      return route.fulfill({ status: 201, json: { ...custom, id: 'auditor-lite', name: 'Lite' } })
    }
    return route.fulfill({ json: [...roles, custom] })
  })
  await page.route('**/api/v1/roles/deployer', (route) => {
    changes.push(route.request())
    return route.request().method() === 'DELETE'
      ? route.fulfill({ status: 204 })
      : route.fulfill({ json: { ...custom, permissions: ['config.read'] } })
  })
  await page.route('**/api/v1/permissions', (route) =>
    route.fulfill({
      json: [
        { name: 'config.read', description: 'Read configuration.' },
        { name: 'config.apply', description: 'Apply configuration.' },
        { name: 'audit.read', description: 'Read the audit trail.' },
      ],
    }),
  )

  await page.goto('/roles')
  await expect(page.getByRole('heading', { name: 'Roles' })).toBeVisible()
  const builtIn = page.getByRole('row', { name: /Administrator/ })
  await expect(builtIn.getByText('Built in')).toBeVisible()
  await expect(builtIn.getByRole('button', { name: 'Edit' })).toHaveCount(0)

  await page.getByRole('button', { name: 'New role' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('auditor-lite')
  await sheet.getByLabel('Name').fill('Lite')
  await sheet.getByLabel('audit.read').click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Created the role Lite')).toBeVisible()
  expect(changes[0]!.postDataJSON()).toEqual({
    id: 'auditor-lite',
    name: 'Lite',
    description: '',
    permissions: ['audit.read'],
  })
  expect(changes[0]!.headers()['x-csrf-token']).toBe(CSRF_TOKEN)

  const row = page.getByRole('row', { name: /Deployer/ })
  await row.getByRole('button', { name: 'Edit' }).click()
  await sheet.getByLabel('config.apply').click()
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Role updated')).toBeVisible()
  expect(changes[1]!.method()).toBe('PUT')
  expect(changes[1]!.postDataJSON()).toEqual({
    name: 'Deployer',
    description: 'Applies configuration.',
    permissions: ['config.read'],
  })

  await row.getByRole('button', { name: 'Delete' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Role deleted')).toBeVisible()
  expect(changes[2]!.method()).toBe('DELETE')
})

test('tokens rotate and other sessions end from the account settings', async ({ page }) => {
  await setUp(page)
  await signIn(page)
  const token = {
    id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a70',
    name: 'ci',
    permissions: ['config.read'],
    created_at: '2026-10-03T08:00:00Z',
    expires_at: '2099-11-02T08:00:00Z',
    last_used_at: null,
    revoked_at: null,
  }
  const requests: Request[] = []
  await page.route('**/api/v1/account/sessions', (route) => {
    if (route.request().method() === 'DELETE') {
      requests.push(route.request())
      return route.fulfill({ json: { ended: 1 } })
    }
    return route.fulfill({
      json: [
        currentSession().session,
        { ...currentSession().session, id: 'other', transport: 'bearer', current: false },
      ],
    })
  })
  await page.route('**/api/v1/account/tokens', (route) => route.fulfill({ json: [token] }))
  await page.route(`**/api/v1/account/tokens/${token.id}/rotate`, (route) => {
    requests.push(route.request())
    return route.fulfill({
      status: 201,
      json: { token: { ...token, id: 'new' }, secret: 'ppat_rotated_secret' },
    })
  })

  await page.goto('/account')
  await page.getByRole('button', { name: 'End other sessions' }).click()
  await expect(page.getByText('Ended 1 session', { exact: true })).toBeVisible()
  await page.getByRole('row', { name: /ci/ }).getByRole('button', { name: 'Rotate' }).click()
  const sheet = page.getByRole('dialog')
  await expect(sheet.getByText('ppat_rotated_secret')).toBeVisible()
  await expect(sheet.getByText('the old one stopped working')).toBeVisible()
  expect(requests.map((request) => request.method())).toEqual(['DELETE', 'POST'])
  expect(requests.every((request) => request.headers()['x-csrf-token'] === CSRF_TOKEN)).toBe(true)
})

test('the sign-in page offers identity providers and explains their failures', async ({ page }) => {
  await setUp(page)
  await page.route('**/api/v1/session', (route) =>
    route.fulfill(problem(401, 'UNAUTHENTICATED', 'log in or present an API token')),
  )
  await page.route('**/api/v1/setup', (route) => route.fulfill({ json: { required: false } }))
  await page.route('**/api/v1/auth/providers', (route) =>
    route.fulfill({ json: [{ id: 'corp', display_name: 'Corporate' }] }),
  )

  await page.goto('/login?next=/sites&sign_in_error=conflict')
  await expect(page.getByRole('alert')).toContainText('A local account already has your username')
  await expect(page.getByRole('link', { name: 'Continue with Corporate' })).toHaveAttribute(
    'href',
    '/api/v1/auth/oidc/corp/start?return_to=%2Fsites',
  )

  await page.goto('/login?sign_in_error=Call%20this%20number')
  await expect(page.getByRole('alert')).toContainText(
    'Signing in through the identity provider failed',
  )
  await expect(page.getByText('Call this number')).toHaveCount(0)
})

test('administrators connect identity providers', async ({ page }) => {
  await setUp(page)
  await signIn(page)
  const changes: Request[] = []
  const corp = {
    id: 'corp',
    display_name: 'Corporate',
    issuer: 'https://id.example',
    client_id: 'panel',
    has_client_secret: true,
    scopes: ['profile', 'email'],
    claims: {
      username: 'preferred_username',
      display_name: 'name',
      email: 'email',
      groups: 'groups',
    },
    group_roles: [{ group: 'ops', role: 'viewer' }],
    create_accounts: true,
    enabled: true,
    created_at: '2026-10-03T00:00:00.000Z',
    updated_at: '2026-10-03T00:00:00.000Z',
  }
  await page.route('**/api/v1/roles', (route) => route.fulfill({ json: roles }))
  await page.route('**/api/v1/identity-providers', (route) => route.fulfill({ json: [corp] }))
  await page.route('**/api/v1/identity-providers/*', (route) => {
    changes.push(route.request())
    if (route.request().method() === 'DELETE') {
      return route.fulfill({ status: 204 })
    }
    const body = route.request().postDataJSON()
    const id = new URL(route.request().url()).pathname.split('/').pop()
    return route.fulfill({
      status: id === 'corp' ? 200 : 201,
      json: { ...corp, ...body, id, has_client_secret: body.client_secret !== null },
    })
  })

  await page.goto('/identity-providers')
  await expect(page.getByRole('heading', { name: 'Sign-in providers' })).toBeVisible()
  const row = page.getByRole('row', { name: /Corporate/ })
  await expect(row.getByText('Creates accounts')).toBeVisible()
  await expect(row.getByText('ops → viewer')).toBeVisible()

  await page.getByRole('button', { name: 'Add provider' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('okta')
  await sheet.getByLabel('Name', { exact: true }).fill('Okta')
  await sheet.getByLabel('Issuer URL').fill('https://example.okta.com')
  await expect(sheet.getByText(/\/api\/v1\/auth\/oidc\/okta\/callback$/)).toBeVisible()
  await sheet.getByLabel('Client ID').fill('panel')
  await sheet.getByLabel('Client secret', { exact: true }).fill('s3cret')
  await sheet.getByRole('button', { name: 'Add mapping' }).click()
  await sheet.getByLabel('Group', { exact: true }).fill('admins')
  await sheet.getByRole('combobox', { name: 'Role' }).click()
  await page.getByRole('option', { name: 'Administrator' }).click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Saved the provider Okta')).toBeVisible()
  expect(new URL(changes[0]!.url()).pathname).toBe('/api/v1/identity-providers/okta')
  expect(changes[0]!.postDataJSON()).toEqual({
    display_name: 'Okta',
    issuer: 'https://example.okta.com',
    client_id: 'panel',
    client_secret: 's3cret',
    scopes: ['profile', 'email'],
    claims: {
      username: 'preferred_username',
      display_name: 'name',
      email: 'email',
      groups: 'groups',
    },
    group_roles: [{ group: 'admins', role: 'administrator' }],
    create_accounts: false,
    enabled: true,
  })
  expect(changes[0]!.headers()['x-csrf-token']).toBe(CSRF_TOKEN)

  await row.getByRole('button', { name: 'Edit' }).click()
  await expect(sheet.getByText('Leave empty to keep the current secret.')).toBeVisible()
  await sheet.getByRole('switch', { name: 'Enabled' }).click()
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved the provider Corporate')).toBeVisible()
  const edited = changes[1]!.postDataJSON()
  expect(edited.enabled).toBe(false)
  expect('client_secret' in edited).toBe(false)

  await row.getByRole('button', { name: 'Delete' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Provider deleted')).toBeVisible()
  expect(changes[2]!.method()).toBe('DELETE')
})
