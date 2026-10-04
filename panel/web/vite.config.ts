import { fileURLToPath, URL } from 'node:url'
import { constants } from 'node:zlib'
import securityHeaders from './security-headers.json' with { type: 'json' }

import { defineConfig, searchForWorkspaceRoot } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { msw } from 'msw/vite'
import { compression, defineAlgorithm } from 'vite-plugin-compression2'

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  // `vite --mode mock` answers API calls in the browser instead (src/mocks).
  plugins: [
    vue(),
    tailwindcss(),
    ...(mode === 'mock' ? [msw()] : []),
    // The management API serves these instead of compressing per request.
    compression({
      algorithms: [
        defineAlgorithm('brotliCompress', {
          params: { [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY },
        }),
        defineAlgorithm('gzip', { level: constants.Z_BEST_COMPRESSION }),
      ],
    }),
  ],
  server: {
    // The management API serves the console in production; in development
    // Vite forwards API calls to it.
    proxy: {
      // `ws` relays WebSocket upgrades, such as following logs.
      '/api': { target: process.env.PANEL_API_URL ?? 'http://127.0.0.1:8080', ws: true },
    },
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
