import type { Component } from 'vue'
import type { RouteRecordRaw } from 'vue-router'

/** One sidebar entry. Every entry carries an icon. */
export interface NavigationItem {
  id: string
  /** i18n key of the visible label. */
  title: string
  icon: Component
  to: string
  /** Shown only to accounts with this permission; the feature's by default. */
  permission?: string
}

export type NavigationGroup = 'operate' | 'configure' | 'administer'

/**
 * A self-contained console feature. The shell renders whatever features
 * register; adding a feature never requires editing the shell.
 */
export interface FeatureModule {
  id: string
  group: NavigationGroup
  /** The API permission its pages need; a route's `meta.permission` overrides it. */
  permission?: string
  routes: RouteRecordRaw[]
  navigation: NavigationItem[]
}
