import { existsSync, statSync } from 'node:fs'
import { expect, test } from './sample'

/** ADR 0023: the first page loads at most 200 KiB of scripts and styles as served. */
const BUDGET = 200 * 1024

/** Bytes the management API sends for `path`: the Brotli file of the build, if it wrote one. */
function served(path: string): number {
  const file = new URL(`../dist${path}`, import.meta.url)
  const brotli = new URL(`${file.href}.br`)
  return statSync(existsSync(brotli) ? brotli : file).size
}

test('the first page loads within its budget', async ({ page }) => {
  // The larger language, so the budget holds for both.
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'zh-CN'))
  await page.goto('/')
  await expect(page.locator('main h1')).toBeVisible()
  await expect(page.locator('main [data-slot="skeleton"], main [aria-busy="true"]')).toHaveCount(0)

  const paths = await page.evaluate(() =>
    performance
      .getEntriesByType('resource')
      .map((entry) => new URL(entry.name))
      .filter((url) => url.origin === location.origin && /\.(js|css)$/.test(url.pathname))
      .map((url) => url.pathname),
  )
  const sizes = paths
    .map((path) => ({ path, bytes: served(path) }))
    .sort((left, right) => right.bytes - left.bytes)
  const total = sizes.reduce((sum, { bytes }) => sum + bytes, 0)
  const report = sizes
    .map(({ path, bytes }) => `${(bytes / 1024).toFixed(1).padStart(6)} KiB ${path}`)
    .join('\n')

  expect(total, `${(total / 1024).toFixed(1)} KiB:\n${report}`).toBeLessThanOrEqual(BUDGET)
})
