# Pingora Panel web console

Vue 3 single-page console served by the management API. It uses shadcn-vue
components on Reka UI, Tailwind CSS v4 and Lucide icons, in a monochrome theme.

## Commands

```sh
pnpm install --frozen-lockfile
pnpm codegen      # typed client from ../panel-api/tests/fixtures/openapi.json
pnpm dev          # Vite dev server; /api is proxied to PANEL_API_URL (default http://127.0.0.1:8080)
pnpm type-check
pnpm lint
pnpm exec vitest run
pnpm test:e2e     # production build with the API mocked, desktop and mobile
pnpm build
```

The generated client in `src/api/generated` is never committed; regenerate it
after the OpenAPI fixture changes.

## Structure

- `src/app`: shell, sidebar and preferences. It renders whatever features register.
- `src/features/<feature>`: routes, navigation entries and views of one capability.
  Add a capability by adding a folder and registering it in `src/features/index.ts`.
- `src/components`: shared presentation such as status, page headers and API failures.
- `src/components/ui`: shadcn-vue components, added and updated with
  `pnpm dlx shadcn-vue@latest add <component>`.
- `src/i18n`: Simplified Chinese and English messages. The English messages
  must match the Chinese catalogue's shape.

## Visual rules

- Theme tokens are achromatic in light and dark mode. Status is shown with an
  icon and text, never by color alone.
- Every navigation entry, action, status and empty state has a Lucide icon.
- Text meets 4.5:1 contrast; input borders and focus rings meet 3:1.
- Fonts are self-hosted, so the console needs no third-party origins.
- Destructive or irreversible commands ask for confirmation.
