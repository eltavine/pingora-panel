import { alertsFeature } from './alerts'
import { approvalsFeature } from './approvals'
import { auditFeature } from './audit'
import { backupsFeature } from './backups'
import { cacheFeature } from './cache'
import { certificatesFeature } from './certificates'
import { configStudioFeature } from './config-studio'
import { containersFeature } from './containers'
import { gatewayFeature } from './gateway'
import { hostFeature } from './host'
import { httpPoliciesFeature } from './http-policies'
import { identityFeature } from './identity'
import { listenersFeature } from './listeners'
import { logsFeature } from './logs'
import { luaFeature } from './lua'
import { publishingFeature } from './publishing'
import { receiptsFeature } from './receipts'
import { revisionsFeature } from './revisions'
import { securityFeature } from './security'
import { siteFilesFeature } from './site-files'
import { sitesFeature } from './sites'
import { trafficFeature } from './traffic'
import type { FeatureModule, NavigationGroup } from './types'
import { upstreamsFeature } from './upstreams'

/** Every console feature, in navigation order. */
export const features: readonly FeatureModule[] = [
  gatewayFeature,
  trafficFeature,
  logsFeature,
  alertsFeature,
  hostFeature,
  containersFeature,
  revisionsFeature,
  publishingFeature,
  approvalsFeature,
  receiptsFeature,
  auditFeature,
  sitesFeature,
  upstreamsFeature,
  listenersFeature,
  securityFeature,
  httpPoliciesFeature,
  cacheFeature,
  certificatesFeature,
  configStudioFeature,
  luaFeature,
  siteFilesFeature,
  backupsFeature,
  identityFeature,
]

export const navigationGroups: readonly { id: NavigationGroup; title: string }[] = [
  { id: 'operate', title: 'nav.operate' },
  { id: 'configure', title: 'nav.configure' },
  { id: 'administer', title: 'nav.administer' },
]
