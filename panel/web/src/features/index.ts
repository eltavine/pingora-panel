import { configStudioFeature } from './config-studio'
import { gatewayFeature } from './gateway'
import { listenersFeature } from './listeners'
import { publishingFeature } from './publishing'
import { receiptsFeature } from './receipts'
import { sitesFeature } from './sites'
import type { FeatureModule, NavigationGroup } from './types'
import { upstreamsFeature } from './upstreams'

/** Every console feature, in navigation order. */
export const features: readonly FeatureModule[] = [
  gatewayFeature,
  publishingFeature,
  receiptsFeature,
  sitesFeature,
  upstreamsFeature,
  listenersFeature,
  configStudioFeature,
]

export const navigationGroups: readonly { id: NavigationGroup; title: string }[] = [
  { id: 'operate', title: 'nav.operate' },
  { id: 'configure', title: 'nav.configure' },
]
