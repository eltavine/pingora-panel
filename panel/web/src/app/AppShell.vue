<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbList,
  BreadcrumbPage,
} from '@/components/ui/breadcrumb'
import { Separator } from '@/components/ui/separator'
import { SidebarInset, SidebarProvider, SidebarTrigger } from '@/components/ui/sidebar'
import DraftStatus from '@/components/DraftStatus.vue'
import AppSidebar from './AppSidebar.vue'
import PreferencesMenu from './PreferencesMenu.vue'
import UserMenu from './UserMenu.vue'

const { t } = useI18n()
const route = useRoute()
const title = computed(() =>
  typeof route.meta.title === 'string' ? t(route.meta.title) : t('app.name'),
)
</script>

<template>
  <SidebarProvider>
    <AppSidebar />
    <SidebarInset>
      <header
        class="bg-background/95 supports-backdrop-filter:bg-background/80 sticky top-0 z-10 flex h-14 shrink-0 items-center gap-2 border-b px-4 backdrop-blur"
      >
        <SidebarTrigger class="-ml-1" :aria-label="t('shell.toggleSidebar')" />
        <Separator
          orientation="vertical"
          class="mr-2 data-[orientation=vertical]:h-4 data-[orientation=vertical]:self-center"
        />
        <Breadcrumb class="min-w-0 flex-1">
          <BreadcrumbList>
            <BreadcrumbItem>
              <BreadcrumbPage class="truncate">{{ title }}</BreadcrumbPage>
            </BreadcrumbItem>
          </BreadcrumbList>
        </Breadcrumb>
        <DraftStatus />
        <PreferencesMenu />
        <UserMenu />
      </header>
      <main class="mx-auto w-full max-w-6xl flex-1 p-4 md:p-6">
        <RouterView />
      </main>
    </SidebarInset>
  </SidebarProvider>
</template>
