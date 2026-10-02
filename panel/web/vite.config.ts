import { fileURLToPath, URL } from 'node:url'
import securityHeaders from './security-headers.json' with { type: 'json' }

import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  server: {
    // The management API serves the console in production; in development
    // Vite forwards API calls to it.
    proxy: { '/api': process.env.PANEL_API_URL ?? 'http://127.0.0.1:8080' },
  },
  // The production headers, so end-to-end tests run under the same policy.
  preview: { headers: securityHeaders },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
})
