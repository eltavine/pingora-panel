import type { Component } from 'vue'
import {
  Ban,
  Binary,
  Box,
  Braces,
  Cable,
  CircleAlert,
  Cpu,
  Database,
  FileCode2,
  Gauge,
  Globe,
  HardDrive,
  HeartPulse,
  KeyRound,
  Layers,
  Lock,
  MemoryStick,
  Network,
  Package,
  PackageCheck,
  Recycle,
  Send,
  Upload,
  Zap,
} from '@lucide/vue'
import type {
  HeaderLine,
  LuaBuiltInModule,
  LuaModules,
  LuaRunOutcome,
  LuaScriptInfo,
} from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** The icon of each library a built-in module stands in for. */
const LIBRARY_ICONS: Record<string, Component> = {
  panel: Box,
  'lua-cjson': Braces,
  luabitop: Binary,
  luajit: Zap,
  'lua-resty-core': Cpu,
  'lua-resty-string': KeyRound,
  'lua-resty-lrucache': Layers,
  'lua-resty-websocket': Cable,
  'lua-resty-lock': Lock,
  'lua-tablepool': Recycle,
  'lua-resty-limit-traffic': Gauge,
  'lua-resty-redis': Database,
  'lua-resty-mysql': HardDrive,
  'lua-resty-memcached': MemoryStick,
  'lua-resty-dns': Globe,
  'lua-resty-http': Send,
  'lua-resty-upload': Upload,
  'lua-upstream-nginx-module': Network,
  'lua-resty-upstream-healthcheck': HeartPulse,
}

export function libraryIcon(library: string): Component {
  return LIBRARY_ICONS[library] ?? Package
}

/** The built-in modules of one library, and how many of them scripts load. */
export interface ModuleGroup {
  library: string
  modules: string[]
  loaded: number
}

/** The names some script loads. */
function loadedNames(scripts: readonly LuaScriptInfo[]): Set<string> {
  return new Set(scripts.flatMap((script) => script.requires))
}

/** Built-in modules by library: libraries the scripts load from first, then by name. */
export function moduleGroups(
  builtIn: readonly LuaBuiltInModule[],
  scripts: readonly LuaScriptInfo[],
): ModuleGroup[] {
  const loaded = loadedNames(scripts)
  const groups = new Map<string, ModuleGroup>()
  for (const module of builtIn) {
    const group = groups.get(module.library) ?? { library: module.library, modules: [], loaded: 0 }
    group.modules.push(module.name)
    if (loaded.has(module.name)) {
      group.loaded += 1
    }
    groups.set(module.library, group)
  }
  return [...groups.values()].sort(
    (left, right) =>
      Number(right.loaded > 0) - Number(left.loaded > 0) ||
      left.library.localeCompare(right.library),
  )
}

export type ModuleOrigin = 'built_in' | 'file' | 'refused' | 'missing'

/** Where `require(name)` finds the module, once the gateway's modules are known. */
export function moduleOrigin(
  name: string,
  modules: LuaModules | undefined,
  scripts: readonly LuaScriptInfo[],
): ModuleOrigin | undefined {
  if (scripts.some((script) => script.module === name)) {
    return 'file'
  }
  if (!modules) {
    return undefined
  }
  if (modules.refused.some((module) => module.name === name)) {
    return 'refused'
  }
  if (modules.built_in.some((module) => module.name === name)) {
    return 'built_in'
  }
  return 'missing'
}

export const ORIGIN_ICONS: Record<ModuleOrigin, Component> = {
  built_in: PackageCheck,
  file: FileCode2,
  refused: Ban,
  missing: CircleAlert,
}

/** The phases a script can be tested in, in the order a connection and its requests run them. */
export const PHASES = [
  'ssl_client_hello',
  'ssl_session_fetch',
  'ssl_cert',
  'ssl_session_store',
  'set',
  'server_rewrite',
  'rewrite',
  'access',
  'precontent',
  'content',
  'balancer',
  'proxy_ssl_cert',
  'proxy_ssl_verify',
  'header_filter',
  'body_filter',
  'log',
] as const

export type Phase = (typeof PHASES)[number]

export const METHODS = ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'] as const

/** How a run's outcome reads; a failure is the only negative one. */
export function outcomeTone(outcome: LuaRunOutcome): StatusTone {
  switch (outcome) {
    case 'failed':
      return 'negative'
    case 'abort':
      return 'warning'
    case 'respond':
      return 'positive'
    default:
      return 'neutral'
  }
}

/** The first twelve hexadecimal digits of a script's SHA-256. */
export function shortVersion(sha256: string): string {
  return sha256.slice(0, 12)
}

/** Whether the script is a file the console can edit, rather than code written in a block. */
export function isFile(script: LuaScriptInfo): boolean {
  return script.id === script.file && script.file.endsWith('.lua')
}

/** The handlers a library's scripts are used by. */
export function handlerCount(scripts: readonly LuaScriptInfo[]): number {
  return scripts.reduce((count, script) => count + script.uses.length, 0)
}

/**
 * Header lines as `Name: value` text, one per line; lines without a colon
 * are reported by their number.
 */
export function parseHeaders(text: string): { headers: HeaderLine[]; invalid: number[] } {
  const headers: HeaderLine[] = []
  const invalid: number[] = []
  text.split('\n').forEach((line, index) => {
    if (!line.trim()) {
      return
    }
    const colon = line.indexOf(':')
    if (colon <= 0) {
      invalid.push(index + 1)
      return
    }
    headers.push({ name: line.slice(0, colon).trim(), value: line.slice(colon + 1).trim() })
  })
  return { headers, invalid }
}

/** Microseconds as people read them. */
export function duration(micros: number): string {
  if (micros < 1000) {
    return `${micros} µs`
  }
  return `${(micros / 1000).toFixed(micros < 10_000 ? 2 : 1)} ms`
}
