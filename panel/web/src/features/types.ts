import type { Component } from 'vue'
import type { RouteRecordRaw } from 'vue-router'

/** One sidebar entry. Every entry carries an icon. */
export interface NavigationItem {
  id: string
  /** i18n key of the visible label. */
  title: string
  icon: Component
  to: string
}

export type NavigationGroup = 'operate' | 'configure'

/**
 * A self-contained console feature. The shell renders whatever features
 * register; adding a feature never requires editing the shell.
 */
export interface FeatureModule {
  id: string
  group: NavigationGroup
  routes: RouteRecordRaw[]
  navigation: NavigationItem[]
}
