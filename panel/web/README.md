# Pingora Panel web console

Vue 3 single-page console served by the management API. It uses shadcn-vue
components on Reka UI, Tailwind CSS v4 and Lucide icons, in a monochrome theme.

## Commands

```sh
pnpm install --frozen-lockfile
pnpm codegen      # typed client from ../panel-api/tests/fixtures/openapi.json
pnpm dev          # Vite dev server; /api is proxied to PANEL_API_URL (default http://127.0.0.1:8080)
pnpm dev:mock     # the same without a backend: Mock Service Worker answers /api in the browser
pnpm type-check
pnpm lint
pnpm exec vitest run
pnpm test:e2e     # production build with the API mocked, desktop and mobile
pnpm build
```

The generated client in `src/api/generated` is never committed; regenerate it
after the OpenAPI fixture changes.

`pnpm dev:mock` previews the console with every permission against
`src/mocks`: hand-written figures for the gateway, traffic, sites and
upstreams, and answers sampled from the OpenAPI contract for every other
operation. Production builds leave the mocks out. End-to-end tests that
import `e2e/sample.ts` run against the same answers through
[`@msw/playwright`](https://github.com/mswjs/playwright).

## Structure

- `src/app`: shell, navigation and preferences. It renders whatever features register;
  a feature offers destinations to the phone navigation bar with `navigationBar`.
- `src/features/<feature>`: routes, navigation entries and views of one capability.
  Add a capability by adding a folder and registering it in `src/features/index.ts`.
  A feature imports no other feature, which ESLint enforces; what several
  features use belongs in `src/lib` or `src/components`.
- `src/components`: shared presentation such as status, page headers and API failures.
- `src/lib`: shared logic, such as how API resources read, form values and
  queries. A change refreshes only the queries of the API tags it can affect;
  the generated query keys carry their operations' tags.
- `src/components/ui`: shadcn-vue components, added and updated with
  `pnpm dlx shadcn-vue@latest add <component>`.
- `src/i18n`: Simplified Chinese and English messages the shell, shared
  components and several features use. The English messages must match the
  Chinese catalogue's shape.
- `src/features/<feature>/locales`: messages only the feature's pages use,
  registered as the feature's `messages` and fetched when one of its pages
  opens. Tests check that both languages define the same keys and that none
  repeats a shared one.

## Visual rules

- Theme tokens are achromatic in light and dark mode. Status is shown with an
  icon and text, never by color alone.
- Every navigation entry, action, status and empty state has a Lucide icon.
- Text meets 4.5:1 contrast; input borders and focus rings meet 3:1.
- Fonts are self-hosted, so the console needs no third-party origins.
- Destructive or irreversible commands ask for confirmation.
