import type { Component } from 'vue'
import { ArrowDownToLine, ArrowUpFromLine, Globe, Server, Shrink } from '@lucide/vue'

/** Icons of what a policy does, by `httpPolicies.effects` key. */
export const effectIcons: Record<string, Component> = {
  request: ArrowUpFromLine,
  response: ArrowDownToLine,
  server: Server,
  cors: Globe,
  compression: Shrink,
}
