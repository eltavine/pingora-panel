import { expect, test, type Page } from '@playwright/test'
import { signIn } from './support'

/** The smallest touch target of ADR 0023 (Apple's default control size). */
const TOUCH_TARGET = 44

const summary = {
  observed_at: '2026-10-04T10:00:00Z',
  window_seconds: 3600,
  requests: 1_200,
  requests_per_second: 0.33,
  statuses: {
    informational: 0,
    success: 1_100,
    redirection: 40,
    client_error: 48,
    server_error: 12,
  },
  latency: { p50: 0.012, p90: 0.08, p95: 0.25, p99: null },
  bytes_received: 2_048,
  bytes_sent: 1_572_864,
  open_connections: 3,
  tls_handshakes: 7,
  upstreams: [],
  routes: [],
  domains: [],
  revision: 7,
  activated_at: '2026-10-04T09:00:00Z',
}

async function setUp(page: Page) {
  await signIn(page)
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route(/\/api\/v1\/sites(\?.*)?$/, (route) =>
    route.fulfill({ json: { items: [], total: 0, next_cursor: null } }),
  )
  await page.route(/\/api\/v1\/traffic(\?.*)?$/, (route) => route.fulfill({ json: summary }))
  await page.route(/\/api\/v1\/traffic\/series/, (route) => route.fulfill({ json: { points: [] } }))
}

/** Visible controls smaller than a touch target, by their hit area. */
async function smallTargets(page: Page): Promise<string[]> {
  return page.evaluate((minimum) => {
    const controls = document.querySelectorAll<HTMLElement>(
      'button, [role="combobox"], input:not([type="hidden"]), a[data-sidebar="menu-button"], nav a',
    )
    const small: string[] = []
    for (const control of controls) {
      if (control.getBoundingClientRect().width === 0) {
        continue
      }
      // A checkbox or switch is tapped through its label.
      const toggle = ['checkbox', 'switch'].includes(control.getAttribute('role') ?? '')
      const target = (toggle && control.closest('label')) || control
      const { width, height } = target.getBoundingClientRect()
      if (height + 0.5 < minimum || width + 0.5 < minimum) {
        const name = control.getAttribute('aria-label') ?? control.textContent?.trim() ?? ''
        small.push(`${control.tagName.toLowerCase()} "${name.slice(0, 30)}" ${width}x${height}`)
      }
    }
    return small
  }, TOUCH_TARGET)
}

test.describe('on touch screens', () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 844 } })

  test('controls are large enough for a finger', async ({ page }) => {
    await setUp(page)

    await page.goto('/traffic')
    await expect(page.getByRole('heading', { name: 'Traffic' })).toBeVisible()

    expect(await smallTargets(page)).toEqual([])
  })
})

test('reduced motion stops transitions and animations', async ({ page }) => {
  await setUp(page)
  await page.emulateMedia({ reducedMotion: 'reduce' })

  await page.goto('/traffic')
  await expect(page.getByRole('heading', { name: 'Traffic' })).toBeVisible()

  const duration = await page
    .getByRole('button', { name: 'Refresh' })
    .evaluate((button) => parseFloat(getComputedStyle(button).transitionDuration))
  expect(duration).toBeLessThan(0.001)
})
