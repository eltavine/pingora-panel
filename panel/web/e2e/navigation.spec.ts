import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

async function setUp(page: Page, permissions = ALL_PERMISSIONS) {
  await page.route('**/api/v1/**', (route) =>
    route.fulfill({ status: 404, json: { title: 'Not Found', status: 404 } }),
  )
  await signIn(page, permissions)
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
}

const navigationBar = (page: Page) => page.getByRole('navigation', { name: 'Main navigation' })
const sidebar = (page: Page) => page.locator('[data-slot="sidebar"][data-state]')

test.describe('in a compact window', () => {
  test.use({ viewport: { width: 390, height: 844 } })

  test('a navigation bar leads to the most used pages and More to every page', async ({ page }) => {
    await setUp(page)
    await page.goto('/')

    const bar = navigationBar(page)
    await expect(bar.getByRole('link')).toHaveText(['Overview', 'Traffic', 'Sites', 'Approvals'])
    await expect(bar.getByRole('link', { name: 'Overview' })).toHaveAttribute(
      'aria-current',
      'page',
    )
    await expect(
      page.getByRole('banner').getByRole('button', { name: 'Toggle sidebar' }),
    ).toBeHidden()

    await bar.getByRole('link', { name: 'Traffic' }).click()
    await expect(page).toHaveURL(/\/traffic$/)
    await expect(bar.getByRole('link', { name: 'Traffic' })).toHaveAttribute('aria-current', 'page')

    const more = bar.getByRole('button', { name: 'More' })
    await more.click()
    const drawer = page.getByRole('dialog', { name: 'Navigation' })
    await drawer.getByRole('link', { name: 'Certificates' }).click()
    await expect(page).toHaveURL(/\/certificates$/)
    await expect(drawer).toBeHidden()
    await expect(more).toHaveAttribute('data-active', '')
    await expect(bar.locator('[aria-current="page"]')).toHaveCount(0)
  })

  test('the navigation bar offers only pages the account may open', async ({ page }) => {
    await setUp(
      page,
      ALL_PERMISSIONS.filter((permission) => permission !== 'config.read'),
    )
    await page.goto('/')

    await expect(navigationBar(page).getByRole('link')).toHaveText([
      'Overview',
      'Traffic',
      'Audit log',
    ])
  })
})

test.describe('in a medium window', () => {
  test.use({ viewport: { width: 700, height: 900 } })

  test('navigation is a rail that opens into the drawer', async ({ page }) => {
    await setUp(page)
    await page.goto('/')

    await expect(navigationBar(page)).toBeHidden()
    await expect(sidebar(page)).toHaveAttribute('data-state', 'collapsed')

    await page.getByRole('banner').getByRole('button', { name: 'Toggle sidebar' }).click()
    await expect(sidebar(page)).toHaveAttribute('data-state', 'expanded')
  })
})

test.describe('in an expanded window', () => {
  test.use({ viewport: { width: 1280, height: 800 } })

  test('navigation is a drawer that narrows to the rail on medium windows', async ({ page }) => {
    await setUp(page)
    await page.goto('/')

    await expect(navigationBar(page)).toBeHidden()
    await expect(sidebar(page)).toHaveAttribute('data-state', 'expanded')

    await page.setViewportSize({ width: 700, height: 800 })
    await expect(sidebar(page)).toHaveAttribute('data-state', 'collapsed')

    await page.setViewportSize({ width: 1280, height: 800 })
    await expect(sidebar(page)).toHaveAttribute('data-state', 'expanded')
  })
})
