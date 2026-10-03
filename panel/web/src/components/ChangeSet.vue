<script setup lang="ts">
import type { Component } from 'vue'
import {
  FileCode2,
  Globe,
  Minus,
  Network,
  PenLine,
  Plus,
  Server,
  ShieldBan,
  ShieldCheck,
} from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import type { Change, Changes, ResourceChange } from '@/api/generated'
import DiffView from '@/components/DiffView.vue'
import { Badge } from '@/components/ui/badge'

defineProps<{ changes: Changes }>()

const { t } = useI18n()

const kinds: Record<string, Component> = {
  sites: Globe,
  upstreams: Server,
  listeners: Network,
  'tls-profiles': ShieldCheck,
  'security-policies': ShieldBan,
}
const changeIcons: Record<Change, Component> = { added: Plus, changed: PenLine, removed: Minus }

const BLOCK = /^[ +-]\s*(?:server|upstream|listener|tls_profile|security_policy)\s+(\S+)\s*\{/

/** The resource's name from the first line of its block, else its path. */
function label(change: ResourceChange): string {
  for (const line of change.diff.split('\n')) {
    const match = BLOCK.exec(line)
    if (match) {
      return match[1]!.replace(/^["']|["']$/g, '')
    }
  }
  return change.resource
}

function kind(change: ResourceChange): string {
  return change.resource.split('/')[0]!
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <section v-if="changes.resources.length" class="flex flex-col gap-2">
      <h3 class="text-muted-foreground text-sm font-medium">
        {{ t('changes.resources', { count: changes.resources.length }) }}
      </h3>
      <ul class="flex flex-col gap-2">
        <li v-for="item in changes.resources" :key="item.resource">
          <details class="group rounded-md border">
            <summary
              class="hover:bg-muted/50 flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-sm"
            >
              <component :is="kinds[kind(item)] ?? FileCode2" class="size-4" aria-hidden="true" />
              <span class="font-medium">{{ label(item) }}</span>
              <span class="text-muted-foreground truncate font-mono text-xs">{{
                item.resource
              }}</span>
              <Badge variant="outline" class="ml-auto gap-1">
                <component :is="changeIcons[item.change]" aria-hidden="true" />
                {{ t(`changes.kind.${item.change}`) }}
              </Badge>
            </summary>
            <div class="border-t p-2">
              <DiffView :diff="item.diff" />
            </div>
          </details>
        </li>
      </ul>
    </section>

    <section v-if="changes.files.length" class="flex flex-col gap-2">
      <h3 class="text-muted-foreground text-sm font-medium">
        {{ t('changes.files', { count: changes.files.length }) }}
      </h3>
      <div v-for="file in changes.files" :key="file.path" class="flex flex-col gap-1.5">
        <p class="flex items-center gap-2 text-sm">
          <FileCode2 class="size-4" aria-hidden="true" />
          <span class="font-mono">{{ file.path }}</span>
          <Badge variant="outline" class="gap-1">
            <component :is="changeIcons[file.change]" aria-hidden="true" />
            {{ t(`changes.kind.${file.change}`) }}
          </Badge>
        </p>
        <DiffView :diff="file.diff" />
      </div>
    </section>
  </div>
</template>
