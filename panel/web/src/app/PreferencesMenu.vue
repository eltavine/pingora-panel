<script setup lang="ts">
import { useColorMode } from '@vueuse/core'
import { Check, Languages, Monitor, Moon, Sun } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { LOCALES, setLocale, type Locale } from '@/i18n'

const { t, locale } = useI18n()
// Transition suppression injects an inline stylesheet, which the console's
// Content Security Policy forbids.
const mode = useColorMode({
  storageKey: 'pingora-panel.color-mode',
  emitAuto: true,
  disableTransition: false,
})

const themes = [
  { value: 'light', icon: Sun, label: 'shell.themeLight' },
  { value: 'dark', icon: Moon, label: 'shell.themeDark' },
  { value: 'auto', icon: Monitor, label: 'shell.themeSystem' },
] as const

const localeNames: Record<Locale, string> = { 'zh-CN': '简体中文', en: 'English' }
</script>

<template>
  <div class="flex items-center gap-1">
    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button variant="ghost" size="icon-sm" :aria-label="t('shell.theme')">
          <Sun class="dark:hidden" aria-hidden="true" />
          <Moon class="hidden dark:block" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>{{ t('shell.theme') }}</DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem v-for="theme in themes" :key="theme.value" @select="mode = theme.value">
          <component :is="theme.icon" aria-hidden="true" />
          {{ t(theme.label) }}
          <Check v-if="mode === theme.value" class="ml-auto" aria-hidden="true" />
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>

    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button variant="ghost" size="icon-sm" :aria-label="t('shell.language')">
          <Languages aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>{{ t('shell.language') }}</DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          v-for="code in LOCALES"
          :key="code"
          :lang="code"
          @select="setLocale(code)"
        >
          {{ localeNames[code] }}
          <Check v-if="locale === code" class="ml-auto" aria-hidden="true" />
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
