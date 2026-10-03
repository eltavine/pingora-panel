<script setup lang="ts">
import { KeyRound, RefreshCw, Trash2 } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { TokenView } from '@/api/generated'
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
import { tokenState } from './presentation'

defineProps<{ tokens: readonly TokenView[]; busy?: boolean; rotatable?: boolean }>()
const emit = defineEmits<{ revoke: [token: TokenView]; rotate: [token: TokenView] }>()
const { t, d } = useI18n()
</script>

<template>
  <div class="rounded-lg border">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('common.name') }}</TableHead>
          <TableHead>{{ t('account.tokenPermissions') }}</TableHead>
          <TableHead>{{ t('account.expires') }}</TableHead>
          <TableHead>{{ t('account.lastUsed') }}</TableHead>
          <TableHead>{{ t('common.status') }}</TableHead>
          <TableHead class="w-20"
            ><span class="sr-only">{{ t('common.actions') }}</span></TableHead
          >
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="token in tokens" :key="token.id">
          <TableCell class="max-w-56">
            <span class="flex items-center gap-2 font-medium">
              <KeyRound class="size-4 shrink-0" aria-hidden="true" />
              <span class="truncate">{{ token.name }}</span>
            </span>
          </TableCell>
          <TableCell class="max-w-72">
            <span class="flex flex-wrap gap-1">
              <Badge
                v-for="permission in token.permissions"
                :key="permission"
                variant="outline"
                class="font-mono"
                >{{ permission }}</Badge
              >
            </span>
          </TableCell>
          <TableCell class="text-muted-foreground text-xs tabular-nums">
            {{ d(new Date(token.expires_at), 'datetime') }}
          </TableCell>
          <TableCell class="text-muted-foreground text-xs tabular-nums">
            {{
              token.last_used_at ? d(new Date(token.last_used_at), 'datetime') : t('account.never')
            }}
          </TableCell>
          <TableCell>
            <Badge :variant="tokenState(token) === 'active' ? 'secondary' : 'outline'">
              {{ t(`account.${tokenState(token)}`) }}
            </Badge>
          </TableCell>
          <TableCell>
            <span v-if="tokenState(token) === 'active'" class="flex justify-end gap-1">
              <Button
                v-if="rotatable"
                variant="ghost"
                size="icon-sm"
                :disabled="busy"
                :aria-label="t('account.rotate')"
                :title="t('account.rotate')"
                @click="emit('rotate', token)"
              >
                <RefreshCw aria-hidden="true" />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                :disabled="busy"
                :aria-label="t('account.revoke')"
                :title="t('account.revoke')"
                @click="emit('revoke', token)"
              >
                <Trash2 aria-hidden="true" />
              </Button>
            </span>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
</template>
