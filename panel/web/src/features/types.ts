import type { Component } from 'vue'
import type { RouteRecordRaw } from 'vue-router'
import type { FeatureMessages } from '@/i18n/types'

/** One sidebar entry. Every entry carries an icon. */
export interface NavigationItem {
  id: string
  /** i18n key of the visible label. */
  title: string
  icon: Component
  to: string
  /** Shown only to accounts with this permission; the feature's by default. */
  permission?: string
  /**
   * Offers the destination to the navigation bar of compact windows, which
   * shows the four lowest the account may open (ADR 0023).
   */
  navigationBar?: number
  /** i18n key of a shorter label where space is tight, as in the navigation bar. */
  shortTitle?: string
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
  /** Messages only its own pages use, fetched when one of them opens. */
  messages?: FeatureMessages
}
