import { defineConfig } from '@hey-api/openapi-ts'

// The reviewed OpenAPI fixture is the single source of truth for the HTTP
// contract; the client is regenerated from it and never committed.
export default defineConfig({
  input: '../panel-api/tests/fixtures/openapi.json',
  output: { path: 'src/api/generated', postProcess: [] },
  plugins: [
    '@hey-api/client-fetch',
    '@hey-api/typescript',
    '@hey-api/sdk',
    // Each query key carries its operation's tags, so a change refreshes
    // only the queries tagged with what it can affect.
    { name: '@tanstack/vue-query', queryKeys: { tags: true } },
  ],
})
