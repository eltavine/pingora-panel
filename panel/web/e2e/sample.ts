import { readFileSync } from 'node:fs'
import { defineNetworkFixture, type NetworkFixture } from '@msw/playwright'
import { test as base } from '@playwright/test'
import { handlers } from '../src/mocks/handlers'
import { createSampler, type Contract } from '../src/mocks/openapi'

const contract = JSON.parse(
  readFileSync(new URL('../../panel-api/tests/fixtures/openapi.json', import.meta.url), 'utf8'),
) as Contract

/** Tests against the management API stand-in of `pnpm dev:mock`, filled with sample data. */
export const test = base.extend<{ network: NetworkFixture }>({
  network: [
    async ({ context }, use) => {
      const network = defineNetworkFixture({
        context,
        handlers: handlers(createSampler(contract)),
        onUnhandledFrame: 'bypass',
      })
      await network.enable()
      await use(network)
      await network.disable()
    },
    { auto: true },
  ],
})

export { expect } from '@playwright/test'
