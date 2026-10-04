/**
 * The console against a stand-in for the management API, for previews
 * without a running stack: `pnpm dev:mock`. Only loaded in the `mock` mode.
 */
export async function startMocks(): Promise<void> {
  const [{ network }, { loadSampler }, { handlers }] = await Promise.all([
    import('virtual:msw'),
    import('./openapi'),
    import('./handlers'),
  ])
  network.configure({ handlers: handlers(await loadSampler()), onUnhandledFrame: 'bypass' })
  await network.enable()
}
