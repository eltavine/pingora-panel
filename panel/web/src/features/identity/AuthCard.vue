<script setup lang="ts">
import { Waypoints } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import PreferencesMenu from '@/app/PreferencesMenu.vue'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

defineProps<{ title: string; description: string }>()
const { t } = useI18n()
</script>

<template>
  <div class="bg-muted/40 flex min-h-svh flex-col">
    <header class="flex items-center justify-between p-4">
      <span class="flex items-center gap-2 text-sm font-semibold">
        <span
          class="bg-primary text-primary-foreground flex size-7 items-center justify-center rounded-md"
          aria-hidden="true"
        >
          <Waypoints class="size-4" />
        </span>
        {{ t('app.name') }}
      </span>
      <PreferencesMenu />
    </header>
    <main class="flex flex-1 items-center justify-center p-4 pb-16">
      <Card class="w-full max-w-sm">
        <CardHeader>
          <CardTitle class="text-xl"
            ><h1>{{ title }}</h1></CardTitle
          >
          <CardDescription>{{ description }}</CardDescription>
        </CardHeader>
        <CardContent>
          <slot />
        </CardContent>
      </Card>
    </main>
  </div>
</template>
