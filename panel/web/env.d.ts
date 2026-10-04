/// <reference types="vite/client" />
/// <reference types="msw/vite/client" />

interface ImportMetaEnv {
  /** Origin of the management API when it differs from the console's origin. */
  readonly VITE_PANEL_API_BASE_URL?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}

declare module 'vue-router' {
  interface RouteMeta {
    /** i18n key of the page title. */
    title?: string
  }
}

export {}
