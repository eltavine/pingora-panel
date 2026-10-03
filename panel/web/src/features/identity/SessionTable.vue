<script setup lang="ts">
import { LogOut, Monitor, SquareTerminal } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { SessionView } from '@/api/generated'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'

defineProps<{ sessions: readonly SessionView[]; busy?: boolean }>()
const emit = defineEmits<{ end: [session: SessionView] }>()
const { t, d } = useI18n()
</script>

<template>
  <div class="rounded-lg border">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('account.client') }}</TableHead>
          <TableHead>{{ t('account.address') }}</TableHead>
          <TableHead>{{ t('account.lastSeen') }}</TableHead>
          <TableHead>{{ t('account.expires') }}</TableHead>
          <TableHead class="w-12"
            ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
          >
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="session in sessions" :key="session.id">
          <TableCell class="max-w-72">
            <span class="flex items-center gap-2">
              <SquareTerminal
                v-if="session.transport === 'bearer'"
                class="size-4 shrink-0"
                aria-hidden="true"
              />
              <Monitor v-else class="size-4 shrink-0" aria-hidden="true" />
              <span class="font-medium">{{ t(`account.transport.${session.transport}`) }}</span>
              <Badge v-if="session.current" variant="secondary">{{ t('account.current') }}</Badge>
            </span>
            <span
              v-if="session.user_agent"
              class="text-muted-foreground block truncate text-xs"
              :title="session.user_agent"
              >{{ session.user_agent }}</span
            >
          </TableCell>
          <TableCell class="font-mono text-xs">{{ session.client_address ?? '—' }}</TableCell>
          <TableCell class="text-muted-foreground text-xs tabular-nums">
            {{ d(new Date(session.last_seen_at), 'datetime') }}
          </TableCell>
          <TableCell class="text-muted-foreground text-xs tabular-nums">
            {{ d(new Date(session.idle_until), 'datetime') }}
          </TableCell>
          <TableCell>
            <Button
              variant="ghost"
              size="icon-sm"
              :disabled="busy"
              :aria-label="t('account.endSession')"
              :title="t('account.endSession')"
              @click="emit('end', session)"
            >
              <LogOut aria-hidden="true" />
            </Button>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
</template>
