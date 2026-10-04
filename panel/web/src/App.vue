<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { useTitle } from '@vueuse/core'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'

const { t } = useI18n()
useTitle(() => t('app.name'))
</script>

<template>
  <TooltipProvider :delay-duration="300">
    <RouterView />
  </TooltipProvider>
  <Toaster :mobile-offset="{ bottom: 'calc(5rem + env(safe-area-inset-bottom))' }" />
</template>
