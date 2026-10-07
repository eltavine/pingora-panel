import type { HttpPoliciesMessages } from './zh-CN'

const en: HttpPoliciesMessages = {
  httpPolicies: {
    title: 'HTTP policies',
    description:
      'Request and response fields, the Server field, cross-origin requests and compression, applied by site or route.',
    listTitle: 'Policies',
    listHint:
      "A site's policy applies to all of its requests; a route's applies after the site's, and its CORS and compression replace the site's.",
    new: 'New policy',
    edit: 'Edit policy',
    id: 'Identifier',
    idHint: 'Lowercase letters, digits and hyphens.',
    idTaken: 'This identifier is already in use',
    does: 'Does',
    usedBy: 'Used by',
    unused: 'Unused',
    nothing: 'Nothing',
    inUse: 'Sites or routes still use this policy',
    saved: 'Saved HTTP policy {id}',
    deleted: 'HTTP policy deleted',
    confirmDeleteTitle: 'Delete HTTP policy {id}?',
    confirmDeleteDetail: 'Only policies no site or route uses can be deleted.',
    emptyTitle: 'No HTTP policies yet',
    emptyDetail:
      'Policies set, add and remove request and response fields, remove or replace the Server field, allow cross-origin requests and compress responses.',
    sections: {
      request: 'Request fields',
      response: 'Response fields',
      server: 'Server field',
      cors: 'Cross-origin requests',
      compression: 'Compression',
    },
    requestHint:
      'Changed before requests go upstream. Values may use $host, $uri, $method, $scheme, $client_ip, $request_id, $http_<name> and $cookie_<name>; Host, framing and forwarding fields stay as the gateway writes them.',
    responseHint:
      'Changed on proxied and generated responses alike; framing fields and Content-Encoding stay as the gateway writes them.',
    operation: 'Change',
    operations: {
      set: 'Set',
      add: 'Add',
      remove: 'Remove',
    },
    fieldName: 'Field',
    fieldNameInvalid: 'A field name such as X-Frame-Options, without spaces',
    fieldValue: 'Value',
    addChange: 'Add field change',
    removeChange: 'Remove field change',
    server: 'Handling',
    serverModes: {
      keep: "Keep the upstream's",
      remove: 'Remove',
      replace: 'Replace',
    },
    serverValue: 'Server value',
    cors: 'Allow cross-origin requests',
    corsHint:
      'The gateway answers preflights itself and lets allowed origins read responses (the Fetch Standard CORS protocol).',
    origins: 'Allowed origins',
    originsHint: 'One per line, such as https://shop.example, https://*.shop.example or *.',
    methods: 'Allowed methods',
    methodsHint: 'Besides GET, HEAD and POST, separated by spaces, such as PUT DELETE.',
    headers: 'Allowed request fields',
    headersHint: 'Separated by spaces, such as X-Api-Key.',
    expose: 'Exposed response fields',
    exposeHint: 'Fields scripts may read, such as X-Request-Id.',
    credentials: 'Allow credentials',
    credentialsHint:
      'Cookies and authorization go along; the origin is echoed back, so * is not allowed with it.',
    maxAge: 'Preflight cache (s)',
    maxAgeHint: 'At most 86400 seconds; browsers decide when empty.',
    compression: 'Compress responses',
    compressionHint:
      'With a coding the client accepts; encoded, partial, HEAD and no-transform responses are sent as they are.',
    codings: 'Codings',
    codingsHint: 'The client picks among them with Accept-Encoding.',
    types: 'Media types',
    typesHint: 'One per line, such as text/html or text/*.',
    minSize: 'Minimum size',
    minSizeHint: 'Smaller responses are sent as they are; empty compresses any size.',
    invalidSize: 'Write a size such as 512, 1k or 1m',
    effects: {
      request: 'Request fields',
      response: 'Response fields',
      server: 'Server',
      cors: 'CORS',
      compression: 'Compression',
    },
  },
}

export default en
