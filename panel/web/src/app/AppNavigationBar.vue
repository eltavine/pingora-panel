<script setup lang="ts">
import { computed } from 'vue'
import { Ellipsis } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useSidebar } from '@/components/ui/sidebar'
import { useNavigation } from './navigation'

const { t } = useI18n()
const { bar, isActive } = useNavigation()
const { openMobile, setOpenMobile } = useSidebar()

const elsewhere = computed(() => !bar.value.some((item) => isActive(item.to)))

const destination =
  'group/destination text-muted-foreground aria-[current=page]:text-foreground data-active:text-foreground focus-visible:ring-ring/50 flex size-full flex-col items-center justify-center gap-1 rounded-md text-xs font-medium outline-none focus-visible:ring-[3px] focus-visible:ring-inset'
const indicator =
  'group-aria-[current=page]/destination:bg-secondary group-data-active/destination:bg-secondary flex h-8 w-14 items-center justify-center rounded-full transition-colors'
</script>

<template>
  <nav
    :aria-label="t('shell.navigationBar')"
    class="bg-background fixed inset-x-0 bottom-0 z-20 border-t pr-[env(safe-area-inset-right)] pb-[env(safe-area-inset-bottom)] pl-[env(safe-area-inset-left)] medium:hidden"
  >
    <ul class="flex h-16">
      <li v-for="item in bar" :key="item.id" class="min-w-0 flex-1">
        <RouterLink
          :to="item.to"
          :aria-current="isActive(item.to) ? 'page' : undefined"
          :class="destination"
        >
          <span :class="indicator">
            <component :is="item.icon" class="size-5" aria-hidden="true" />
          </span>
          <span class="max-w-full truncate px-1">{{ t(item.shortTitle ?? item.title) }}</span>
        </RouterLink>
      </li>
      <li class="min-w-0 flex-1">
        <button
          type="button"
          aria-haspopup="dialog"
          :aria-expanded="openMobile"
          :data-active="elsewhere ? '' : undefined"
          :class="destination"
          @click="setOpenMobile(true)"
        >
          <span :class="indicator">
            <Ellipsis class="size-5" aria-hidden="true" />
          </span>
          <span class="max-w-full truncate px-1">{{ t('shell.more') }}</span>
        </button>
      </li>
    </ul>
  </nav>
</template>
