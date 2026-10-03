import type { Component } from 'vue'
import { Construction, CornerUpRight, FolderOpen, MessageSquareText, Network } from '@lucide/vue'
import type { Action, SiteKind, SiteStatus } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

export const kindIcons: Record<SiteKind, Component> = {
  reverse_proxy: Network,
  static: FolderOpen,
  redirect: CornerUpRight,
  maintenance: Construction,
}

export const actionIcons: Record<Action['type'], Component> = {
  proxy: Network,
  static: FolderOpen,
  redirect: CornerUpRight,
  respond: MessageSquareText,
}

export const statusTones: Record<SiteStatus, StatusTone> = {
  running: 'positive',
  stopped: 'neutral',
  abnormal: 'warning',
  deleted: 'negative',
}

/** One host per line or comma; blank lines and `#` comments are ignored. */
export function parseHosts(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((line) => line.split('#')[0]!.trim())
    .filter((line) => line.length > 0)
}
