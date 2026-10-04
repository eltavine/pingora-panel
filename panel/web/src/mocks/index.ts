import contractUrl from '../../../panel-api/tests/fixtures/openapi.json?url'
import type { Contract } from './openapi'

/**
 * The console against a stand-in for the management API, for previews
 * without a running stack: `pnpm dev:mock`. Only loaded in the `mock` mode.
 */
export async function startMocks(): Promise<void> {
  const [{ network }, { createSampler }, { handlers }, contract] = await Promise.all([
    import('virtual:msw'),
    import('./openapi'),
    import('./handlers'),
    fetch(contractUrl).then((response) => response.json() as Promise<Contract>),
  ])
  network.configure({ handlers: handlers(createSampler(contract)), onUnhandledFrame: 'bypass' })
  await network.enable()
}
