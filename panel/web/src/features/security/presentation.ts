import type { Component } from 'vue'
import {
  Bot,
  FolderX,
  Gauge,
  KeyRound,
  Link2,
  ListFilter,
  Network,
  Ruler,
  Timer,
  Users,
} from '@lucide/vue'

/** Icons of what a policy checks, by `security.restrictions` key. */
export const restrictionIcons: Record<string, Component> = {
  networks: Network,
  methods: ListFilter,
  paths: FolderX,
  userAgents: Bot,
  referers: Link2,
  password: KeyRound,
  sizes: Ruler,
  bodyTimeout: Timer,
  rates: Gauge,
  concurrency: Users,
}
