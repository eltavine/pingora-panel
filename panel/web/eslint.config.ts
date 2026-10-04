import { readdirSync } from 'node:fs'
import type { Linter } from 'eslint'
import { globalIgnores } from 'eslint/config'
import { defineConfigWithVueTs, vueTsConfigs } from '@vue/eslint-config-typescript'
import pluginVue from 'eslint-plugin-vue'
import pluginPlaywright from 'eslint-plugin-playwright'
import pluginVitest from '@vitest/eslint-plugin'
import pluginOxlint from 'eslint-plugin-oxlint'
import skipFormatting from 'eslint-config-prettier/flat'

// To allow more languages other than `ts` in `.vue` files, uncomment the following lines:
// import { configureVueProject } from '@vue/eslint-config-typescript'
// configureVueProject({ scriptLangs: ['ts', 'tsx'] })
// More info at https://github.com/vuejs/eslint-config-typescript/#advanced-setup

const ACROSS_FEATURES =
  'A feature imports no other feature: what features share belongs in src/lib or src/components.'

/**
 * Each feature imports only itself, the feature registry's types and shared
 * modules, so features can change and go independently. Relative imports may
 * not leave the feature either; features nest one directory deep.
 */
const featureBoundaries = readdirSync(new URL('./src/features/', import.meta.url), {
  withFileTypes: true,
})
  .filter((entry) => entry.isDirectory())
  .flatMap(({ name }) => {
    const others = { regex: `^@/features/(?!(?:${name}|types)(?:/|$))`, message: ACROSS_FEATURES }
    const boundary = (files: string, parent: string): Linter.Config => ({
      name: `app/feature-boundaries/${name}${parent === '../' ? '' : '/nested'}`,
      files: [files],
      rules: {
        'no-restricted-imports': [
          'error',
          {
            patterns: [
              others,
              { regex: `^${parent.replaceAll('.', '\\.')}`, message: ACROSS_FEATURES },
            ],
          },
        ],
      },
    })
    return [
      boundary(`src/features/${name}/*.{ts,vue}`, '../'),
      boundary(`src/features/${name}/*/**/*.{ts,vue}`, '../../'),
    ]
  })

export default defineConfigWithVueTs(
  {
    name: 'app/files-to-lint',
    files: ['**/*.{vue,ts,mts,tsx}'],
  },

  // Generated client code is regenerated from the OpenAPI contract.
  globalIgnores(['**/dist/**', '**/dist-ssr/**', '**/coverage/**', 'src/api/generated/**']),

  ...pluginVue.configs['flat/essential'],
  vueTsConfigs.recommended,

  {
    ...pluginPlaywright.configs['flat/recommended'],
    files: ['e2e/**/*.{test,spec}.{js,ts,jsx,tsx}'],
  },

  {
    ...pluginVitest.configs.recommended,
    files: ['src/**/__tests__/*'],
  },

  {
    // shadcn-vue components keep the registry's file names so the CLI can
    // update them in place.
    name: 'app/generated-ui-components',
    files: ['src/components/ui/**/*.vue'],
    rules: { 'vue/multi-word-component-names': 'off' },
  },

  ...featureBoundaries,

  ...pluginOxlint.buildFromOxlintConfigFile('.oxlintrc.json'),

  skipFormatting,
)
