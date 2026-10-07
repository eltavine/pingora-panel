import type { CacheMessages } from './zh-CN'

const en: CacheMessages = {
  cache: {
    title: 'Cache',
    description:
      "Proxied responses kept in the gateway's memory by policy: what tells them apart, how long they stay fresh, what bypasses them, and purges.",
    listTitle: 'Cache policies',
    listHint:
      "A site's policy caches its proxied responses; a route's replaces the site's, and a route can stay out of the cache.",
    new: 'New policy',
    edit: 'Edit policy',
    formHint:
      'What tells responses apart, how long they stay fresh, what bypasses the cache and when stale responses are served.',
    id: 'Identifier',
    idHint: 'Lowercase letters, digits and hyphens; not off.',
    idTaken: 'This identifier is already in use',
    enabled: 'Cache responses',
    enabledHint: 'A disabled policy caches nothing for the sites and routes naming it.',
    disabled: 'Disabled',
    lifetime: 'Fresh for',
    originsLifetime: "Origin's",
    bypassCount: '{count} bypass | {count} bypasses',
    usedBy: 'Used by',
    unused: 'Unused',
    inUse: 'Sites or routes still use this policy',
    saved: 'Saved cache policy {id}',
    deleted: 'Cache policy deleted',
    confirmDeleteTitle: 'Delete cache policy {id}?',
    confirmDeleteDetail: 'Only policies no site or route uses can be deleted.',
    emptyTitle: 'No cache policies yet',
    emptyDetail:
      'Policies keep proxied responses in memory for as long as the origin or the policy says, and answer with them without asking the upstream.',
    sections: {
      lifetime: 'Freshness',
      key: 'Key and variants',
      bypass: 'Bypass',
      stale: 'Stale responses',
    },
    honorOrigin: "Follow the origin's Cache-Control and Expires",
    honorOriginHint: 'Off lets this policy alone decide what is stored and for how long.',
    ttl: 'Fresh for',
    ttlHint:
      'For 200, 203, 204, 300, 301 and 308 responses whose origin does not say, such as 10m; when empty, only responses the origin gives a lifetime are stored.',
    statusTtls: 'By status',
    statusTtlsHint:
      'Statuses with a lifetime of their own, ahead of the one above; 0 keeps them out.',
    statuses: 'Statuses',
    statusTtl: 'Fresh for',
    addStatusTtl: 'Add statuses',
    key: 'Key',
    keyHint:
      'Request variables that tell stored responses apart; $scheme$host$request_uri when empty.',
    vary: 'Vary by',
    varyHint:
      "Request fields whose values keep responses apart, besides those the response's Vary names.",
    bypassHint:
      'Requests meeting any of these neither use nor fill the cache, such as those with a session cookie.',
    staleHint:
      'How long a stale response is served while it is revalidated, or when the upstream fails, unless the origin says (RFC 5861).',
    staleWhileRevalidate: 'While revalidating',
    staleIfError: 'When the upstream fails',
    maxObjectSize: 'Largest response',
    maxObjectSizeHint: 'Larger responses pass without being stored; 8m when empty, at most 64m.',
    statusHeader: 'Send Cache-Status',
    statusHeaderHint: 'Responses say whether they came from the cache (RFC 9211).',
    problems: {
      ttl: 'Write a duration such as 30s, 10m or 1h',
      statuses: 'Write statuses such as 404 410 with a duration such as 1m, or 0',
      vary: 'Write field names such as accept-language',
      bypass: 'Complete or remove the unfinished conditions',
      stale: 'Write durations such as 30s or 5m',
      objectSize: 'Write a size such as 8m, at most 64m',
    },
    stats: {
      title: 'Cache use',
      hint: 'What the gateway keeps, and how its lookups went for each site since it started.',
      size: 'Size',
      sizeOf: '{used} of {max}',
      entries: 'Entries',
      since: 'Counted since',
      noLookups: 'No site has used the cache yet.',
      site: 'Site',
      hitRatio: 'Hit ratio',
      served: 'From cache',
      fetched: 'From upstream',
      bypassed: 'Bypassed',
    },
    purge: {
      all: 'Purge everything',
      allTitle: 'Purge the whole cache?',
      allDetail: 'Every stored response is fetched again when next asked for.',
      site: 'Purge site',
      siteTitle: 'Purge the cache of {site}?',
      siteDetail: "The site's stored responses are fetched again when next asked for.",
      urls: 'URLs to purge',
      urlsHint: 'One absolute URL per line; every variant stored for it goes.',
      urlsAction: 'Purge URLs',
      done: 'Purged the cache',
      keys: 'Purged {count} key | Purged {count} keys',
    },
    store: {
      title: 'Cache store',
      hint: "How much the gateway's in-memory cache keeps; a new size empties it once the configuration is applied.",
      size: 'Size',
      sizeHint: 'Such as 512m or 2g, between 1m and 64g; 256m when empty.',
      invalid: 'Write a size between 1m and 64g',
      saved: 'Saved the cache size; it applies with the configuration',
    },
  },
}

export default en
