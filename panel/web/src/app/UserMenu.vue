<script setup lang="ts">
import { computed } from 'vue'
import { CircleUser, LogOut, UserCog } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { signOut, useSession } from '@/lib/session'

const { t } = useI18n()
const router = useRouter()
const { session } = useSession()
const name = computed(
  () => session.value?.account.display_name || session.value?.account.username || '',
)

async function logOut() {
  try {
    await signOut()
  } finally {
    toast.success(t('auth.loggedOut'))
    await router.replace({ name: 'login' })
  }
}
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button variant="ghost" size="icon-sm" :aria-label="t('auth.signedInAs', { name })">
        <CircleUser aria-hidden="true" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="min-w-56">
      <DropdownMenuLabel class="flex flex-col">
        <span class="truncate">{{ name }}</span>
        <span
          v-if="session?.account.display_name"
          class="text-muted-foreground truncate font-mono text-xs font-normal"
          >{{ session.account.username }}</span
        >
      </DropdownMenuLabel>
      <DropdownMenuSeparator />
      <DropdownMenuItem @select="router.push('/account')">
        <UserCog aria-hidden="true" />
        {{ t('nav.account') }}
      </DropdownMenuItem>
      <DropdownMenuItem @select="logOut">
        <LogOut aria-hidden="true" />
        {{ t('auth.logout') }}
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
