import { fileURLToPath, URL } from 'node:url'
import securityHeaders from './security-headers.json' with { type: 'json' }

import { defineConfig, searchForWorkspaceRoot } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { msw } from 'msw/vite'

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  // `vite --mode mock` answers API calls in the browser instead (src/mocks).
  plugins: [vue(), tailwindcss(), ...(mode === 'mock' ? [msw()] : [])],
  server: {
    // The management API serves the console in production; in development
    // Vite forwards API calls to it.
    proxy: { '/api': process.env.PANEL_API_URL ?? 'http://127.0.0.1:8080' },
    // The mock reads the reviewed OpenAPI contract next to the console.
    fs: {
      allow: [
        searchForWorkspaceRoot(process.cwd()),
        fileURLToPath(new URL('../panel-api/tests/fixtures', import.meta.url)),
      ],
    },
  },
  // The production headers, so end-to-end tests run under the same policy.
  preview: { headers: securityHeaders },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
}))
