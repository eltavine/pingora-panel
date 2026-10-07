import type { Component } from 'vue'
import {
  ArrowLeftToLine,
  Braces,
  Construction,
  CornerUpRight,
  FileCode,
  FileBraces,
  Files,
  FileType,
  FolderLock,
  FolderOpen,
  Hourglass,
  Image,
  ImageOff,
  Layers,
  LayoutList,
  MessageSquareText,
  Network,
  Package,
  PencilLine,
  RefreshCcw,
  Regex,
  Replace,
  Scissors,
  SearchX,
  ServerCrash,
  ShieldCheck,
  ShieldOff,
  ShieldX,
  SquareDashed,
  Waypoints,
} from '@lucide/vue'
import type { Action, DirectoryListing, SiteKind, SiteStatus } from '@/api/generated'
import type { FaviconChoice, PageKind, RobotsChoice, RoutePagesMode } from './pages'
import type { RewriteKind } from './rewrites'
import type { CacheMode, CachePreset } from './statics'
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
  internal_redirect: Waypoints,
}

export const rewriteIcons: Record<RewriteKind, Component> = {
  strip_prefix: Scissors,
  add_prefix: ArrowLeftToLine,
  set_uri: Replace,
  rewrite: Regex,
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

export const pageIcons: Record<PageKind, Component> = {
  body: FileCode,
  file: FileType,
  redirect: CornerUpRight,
}

/** The errors pages are first given for, by the icon that tells them apart. */
export const presetIcons: Record<number, Component> = {
  404: SearchX,
  403: ShieldX,
  502: ServerCrash,
  503: Construction,
}

export const pagesModeIcons: Record<RoutePagesMode, Component> = {
  site: Layers,
  own: FileCode,
  none: SquareDashed,
}

export const robotsIcons: Record<RobotsChoice, Component> = {
  routes: Waypoints,
  allow_all: ShieldCheck,
  disallow_all: ShieldOff,
  custom: PencilLine,
}

export const faviconIcons: Record<FaviconChoice, Component> = {
  routes: Waypoints,
  no_content: ImageOff,
  file: Image,
  redirect: CornerUpRight,
}

export const listingIcons: Record<DirectoryListing, Component> = {
  off: FolderLock,
  html: LayoutList,
  json: FileBraces,
}

export const cacheIcons: Record<CacheMode, Component> = {
  max_age: Hourglass,
  no_cache: RefreshCcw,
}

export const cachePresetIcons: Record<CachePreset, Component> = {
  assets: Package,
  html: FileCode,
  everything: Files,
}
