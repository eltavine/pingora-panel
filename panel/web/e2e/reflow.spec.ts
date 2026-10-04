import type { Page } from '@playwright/test'
import { expect, test } from './sample'

/** WCAG 2.2 SC 1.4.10's narrowest width, a medium window with the rail and a small desktop. */
const WIDTHS = [320, 700, 1024]

/** Every page the navigation leads to. */
async function destinations(page: Page): Promise<string[]> {
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.goto('/')
  const links = page.locator('[data-slot="sidebar"] a[href]')
  await expect.poll(() => links.count()).toBeGreaterThan(10)
  const paths = await links.evaluateAll((anchors) =>
    anchors.map((anchor) => new URL((anchor as HTMLAnchorElement).href).pathname),
  )
  return [...new Set(paths)]
}

for (const width of WIDTHS) {
  test(`every page fits ${width} px without scrolling sideways`, async ({ page }) => {
    await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
    const paths = await destinations(page)

    await page.setViewportSize({ width, height: 900 })
    for (const path of paths) {
      await page.goto(path)
      await expect(page.locator('main h1')).toBeVisible()
      await expect(
        page.locator('main [data-slot="skeleton"], main [aria-busy="true"]'),
      ).toHaveCount(0)
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      )
      expect.soft(overflow, `${path} overflows by ${overflow} px`).toBeLessThanOrEqual(0)
    }
  })
}
