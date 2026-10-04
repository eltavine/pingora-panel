<script setup lang="ts">
import { watch } from 'vue'
import { Waypoints } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  useSidebar,
} from '@/components/ui/sidebar'
import { useNavigation } from './navigation'

const { t } = useI18n()
const route = useRoute()
const { groups, isActive } = useNavigation()
const { setOpenMobile } = useSidebar()

watch(
  () => route.fullPath,
  () => setOpenMobile(false),
)
</script>

<template>
  <Sidebar
    collapsible="icon"
    :mobile-title="t('shell.navigation')"
    :mobile-description="t('shell.navigationDescription')"
  >
    <SidebarHeader>
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton size="lg" as-child>
            <RouterLink to="/">
              <div
                class="bg-sidebar-primary text-sidebar-primary-foreground flex aspect-square size-8 items-center justify-center rounded-lg"
                aria-hidden="true"
              >
                <Waypoints class="size-4" />
              </div>
              <div class="grid flex-1 text-left text-sm leading-tight">
                <span class="truncate font-semibold">{{ t('app.name') }}</span>
                <span class="text-muted-foreground truncate text-xs">{{ t('app.tagline') }}</span>
              </div>
            </RouterLink>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    </SidebarHeader>
    <SidebarContent>
      <SidebarGroup v-for="group in groups" :key="group.id">
        <SidebarGroupLabel>{{ t(group.title) }}</SidebarGroupLabel>
        <SidebarMenu>
          <SidebarMenuItem v-for="item in group.items" :key="item.id">
            <SidebarMenuButton as-child :is-active="isActive(item.to)" :tooltip="t(item.title)">
              <RouterLink :to="item.to" :aria-current="isActive(item.to) ? 'page' : undefined">
                <component :is="item.icon" aria-hidden="true" />
                <span>{{ t(item.title) }}</span>
              </RouterLink>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarGroup>
    </SidebarContent>
    <SidebarRail :aria-label="t('shell.toggleSidebar')" :title="t('shell.toggleSidebar')" />
  </Sidebar>
</template>
