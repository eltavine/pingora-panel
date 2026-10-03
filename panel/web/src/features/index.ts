import { approvalsFeature } from './approvals'
import { auditFeature } from './audit'
import { certificatesFeature } from './certificates'
import { configStudioFeature } from './config-studio'
import { gatewayFeature } from './gateway'
import { identityFeature } from './identity'
import { listenersFeature } from './listeners'
import { publishingFeature } from './publishing'
import { receiptsFeature } from './receipts'
import { revisionsFeature } from './revisions'
import { securityFeature } from './security'
import { sitesFeature } from './sites'
import type { FeatureModule, NavigationGroup } from './types'
import { upstreamsFeature } from './upstreams'

/** Every console feature, in navigation order. */
export const features: readonly FeatureModule[] = [
  gatewayFeature,
  revisionsFeature,
  publishingFeature,
  approvalsFeature,
  receiptsFeature,
  auditFeature,
  sitesFeature,
  upstreamsFeature,
  listenersFeature,
  securityFeature,
  certificatesFeature,
  configStudioFeature,
  identityFeature,
]

export const navigationGroups: readonly { id: NavigationGroup; title: string }[] = [
  { id: 'operate', title: 'nav.operate' },
  { id: 'configure', title: 'nav.configure' },
  { id: 'administer', title: 'nav.administer' },
]
