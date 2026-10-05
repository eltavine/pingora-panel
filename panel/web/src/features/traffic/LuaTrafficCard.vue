<script setup lang="ts">
import { computed } from 'vue'
import { Braces, CircleX, Gauge, MemoryStick, Timer } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { LuaTrafficItem } from '@/api/generated'
import StatTile from '@/components/StatTile.vue'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { formatters } from '@/lib/format'
import { runTime } from './presentation'

const props = defineProps<{ lua: LuaTrafficItem }>()

const { t, locale } = useI18n()
const format = computed(() => formatters(locale.value))
const failed = computed(() =>
  Object.values(props.lua.failures).reduce((sum, value) => sum + value, 0),
)
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <Braces class="size-4" aria-hidden="true" />{{ t('traffic.lua.title') }}
      </CardTitle>
      <CardDescription>{{ t('traffic.lua.description') }}</CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <div class="grid grid-cols-2 gap-3 lg:grid-cols-5">
        <StatTile
          :icon="Braces"
          :label="t('traffic.lua.runs')"
          :value="format.count(lua.runs)"
          passive
        />
        <StatTile
          :icon="CircleX"
          :label="t('traffic.lua.failed')"
          :value="format.count(failed)"
          passive
        />
        <StatTile
          :icon="Timer"
          :label="t('traffic.lua.slow')"
          :value="format.count(lua.slow_runs)"
          passive
        />
        <StatTile
          :icon="Gauge"
          :label="t('traffic.lua.p95')"
          :value="runTime(lua.latency.p95)"
          passive
        />
        <StatTile
          :icon="MemoryStick"
          :label="t('traffic.lua.memory')"
          :value="format.bytes(lua.memory_bytes)"
          passive
        />
      </div>
      <div v-if="failed > 0" class="flex flex-wrap items-center gap-2 text-sm">
        <span class="text-muted-foreground">{{ t('traffic.lua.why') }}</span>
        <Badge
          v-for="(count, kind) in lua.failures"
          :key="kind"
          variant="outline"
          class="font-mono"
        >
          {{ kind }} · {{ format.count(count) }}
        </Badge>
      </div>
      <div v-if="lua.handlers.length" class="overflow-x-auto rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{{ t('traffic.site') }}</TableHead>
              <TableHead>{{ t('traffic.route') }}</TableHead>
              <TableHead>{{ t('traffic.lua.phase') }}</TableHead>
              <TableHead class="text-right">{{ t('traffic.lua.runs') }}</TableHead>
              <TableHead class="text-right">{{ t('traffic.lua.failed') }}</TableHead>
              <TableHead class="text-right">{{ t('traffic.lua.slow') }}</TableHead>
              <TableHead class="text-right">P95</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="handler in lua.handlers"
              :key="`${handler.site}/${handler.route}/${handler.phase}`"
            >
              <TableCell>{{ handler.site || '—' }}</TableCell>
              <TableCell class="font-medium">{{ handler.route || '—' }}</TableCell>
              <TableCell class="font-mono text-xs">{{ handler.phase }}</TableCell>
              <TableCell class="text-right tabular-nums">{{
                format.count(handler.runs)
              }}</TableCell>
              <TableCell class="text-right tabular-nums">
                {{ format.count(handler.failures) }}
              </TableCell>
              <TableCell class="text-right tabular-nums">
                {{ format.count(handler.slow_runs) }}
              </TableCell>
              <TableCell class="text-right tabular-nums">{{ runTime(handler.p95) }}</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </CardContent>
  </Card>
</template>
