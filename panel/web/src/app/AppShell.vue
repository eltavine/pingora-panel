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
import AppNavigationBar from './AppNavigationBar.vue'
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
        class="bg-background/95 supports-backdrop-filter:bg-background/80 sticky top-0 z-10 flex min-h-[calc(--spacing(14)+env(safe-area-inset-top))] shrink-0 items-center gap-2 border-b pt-[env(safe-area-inset-top)] pr-[max(--spacing(4),env(safe-area-inset-right))] pl-[max(--spacing(4),env(safe-area-inset-left))] backdrop-blur medium:pl-4"
      >
        <SidebarTrigger class="-ml-1 max-medium:hidden" :aria-label="t('shell.toggleSidebar')" />
        <Separator
          orientation="vertical"
          class="mr-2 data-[orientation=vertical]:h-4 data-[orientation=vertical]:self-center max-medium:hidden"
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
      <main
        class="mx-auto w-full max-w-6xl flex-1 pt-4 pr-[max(--spacing(4),env(safe-area-inset-right))] pb-[calc(--spacing(20)+env(safe-area-inset-bottom))] pl-[max(--spacing(4),env(safe-area-inset-left))] medium:pt-6 medium:pr-[max(--spacing(6),env(safe-area-inset-right))] medium:pb-[max(--spacing(6),env(safe-area-inset-bottom))] medium:pl-6"
      >
        <RouterView />
      </main>
    </SidebarInset>
    <AppNavigationBar />
  </SidebarProvider>
</template>
