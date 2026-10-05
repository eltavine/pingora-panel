import type { Component } from 'vue'
import {
  ArrowRightLeft,
  Ban,
  Bot,
  Braces,
  Construction,
  Cookie,
  CornerUpRight,
  FileType,
  FolderOpen,
  Globe,
  Heading,
  Link,
  ListChecks,
  MessageSquareText,
  Network,
  Split,
  Variable,
} from '@lucide/vue'
import type { Action, RouteCondition, SiteKind, SiteStatus } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

export const kindIcons: Record<SiteKind, Component> = {
  reverse_proxy: Network,
  static: FolderOpen,
  redirect: CornerUpRight,
  maintenance: Construction,
  script: Braces,
}

export const actionIcons: Record<Action['type'], Component> = {
  proxy: Network,
  static: FolderOpen,
  redirect: CornerUpRight,
  respond: MessageSquareText,
  lua: Braces,
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

export const conditionIcons: Record<RouteCondition['kind'], Component> = {
  method: ArrowRightLeft,
  host: Globe,
  header: Heading,
  query: Variable,
  cookie: Cookie,
  client: Network,
  user_agent: Bot,
  referer: Link,
  content_type: FileType,
  any: Split,
  all: ListChecks,
  not: Ban,
}
