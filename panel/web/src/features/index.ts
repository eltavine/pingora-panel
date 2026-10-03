import { auditFeature } from './audit'
import { configStudioFeature } from './config-studio'
import { gatewayFeature } from './gateway'
import { identityFeature } from './identity'
import { listenersFeature } from './listeners'
import { publishingFeature } from './publishing'
import { receiptsFeature } from './receipts'
import { revisionsFeature } from './revisions'
import { sitesFeature } from './sites'
import type { FeatureModule, NavigationGroup } from './types'
import { upstreamsFeature } from './upstreams'

/** Every console feature, in navigation order. */
export const features: readonly FeatureModule[] = [
  gatewayFeature,
  revisionsFeature,
  publishingFeature,
  receiptsFeature,
  auditFeature,
  sitesFeature,
  upstreamsFeature,
  listenersFeature,
  configStudioFeature,
  identityFeature,
]

export const navigationGroups: readonly { id: NavigationGroup; title: string }[] = [
  { id: 'operate', title: 'nav.operate' },
  { id: 'configure', title: 'nav.configure' },
  { id: 'administer', title: 'nav.administer' },
]
