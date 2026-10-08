import type { IdentityMessages } from './zh-CN'

const en: IdentityMessages = {
  roles: {
    title: 'Roles',
    description:
      'A role is a set of permissions; built-in roles ship with the panel and cannot change.',
    new: 'New role',
    edit: 'Edit role',
    id: 'Identifier',
    idHint: 'Lowercase letters, digits, dots, underscores or hyphens; it cannot change later.',
    name: 'Name',
    summary: 'Description',
    permissions: 'Permissions',
    builtIn: 'Built in',
    custom: 'Custom',
    created: 'Created the role {name}',
    updated: 'Role updated',
    deleted: 'Role deleted',
    deleteTitle: 'Delete the role {name}?',
    deleteDetail: 'Only roles no account holds can be deleted.',
  },
  grants: {
    title: 'Grants',
    description:
      'Roles given besides the account\u2019s own, for a site group, one site or under conditions; scopes limit configuration permissions only.',
    add: 'Add grant',
    give: 'Grant',
    empty: 'No grants besides the account\u2019s roles.',
    role: 'Role',
    scope: 'Scope',
    siteGroup: 'Site group',
    site: 'One site',
    everything: 'Everything',
    groupName: 'Site group name',
    siteId: 'Site ID',
    scopeHint:
      'Counts for configuration permissions on these sites; other permissions only with the scope Everything.',
    untilLabel: 'Until (optional)',
    networks: 'From networks (optional)',
    networksHint: 'Separated by commas or spaces, such as 10.0.0.0/8',
    windows: 'Time windows',
    windowsHint: 'Counts only within these windows; always when there are none.',
    groupScope: 'site group {group}',
    siteScope: 'site {site}',
    until: 'until {at}',
    from: 'from {networks}',
    always: 'always',
    revoke: 'Revoke grant',
    created: 'Grant added',
    revoked: 'Grant revoked',
  },
  workloads: {
    title: 'Workload identities',
    description:
      'Let programs such as CI jobs exchange short-lived tokens from their issuer for short sessions of a service account, with no stored secret.',
    new: 'Add workload identity',
    edit: 'Edit workload identity',
    empty: 'No workload identities yet.',
    id: 'Identifier',
    account: 'Service account',
    accountHint: 'The workload acts as this service account.',
    chooseAccount: 'Choose a service account',
    issuer: 'Issuer URL',
    issuerHint:
      'Exactly as its tokens name it, such as https://token.actions.githubusercontent.com for GitHub Actions',
    audience: 'Audience',
    subject: 'Subject',
    subjectHint: 'Exactly, or a prefix ending in *, such as repo:shop/site:ref:refs/heads/*',
    claims: 'Further claims',
    claimsHint: 'The token must carry these claims with these values.',
    claimName: 'Claim',
    claimValue: 'Value',
    addClaim: 'Add claim',
    removeClaim: 'Remove claim',
    sessionMinutes: 'Session length (minutes, 5–60)',
    enabled: 'Enabled',
    disabled: 'Disabled',
    minutes: '{count} min',
    saved: 'Saved the workload identity {id}',
    deleted: 'Workload identity deleted',
    deleteTitle: 'Delete the workload identity {id}?',
    deleteDetail: 'Sessions it opened end when they run out.',
  },
  permissions: {
    gateway_read: 'Read the gateway status, data plane and upstream health',
    gateway_operate: 'Reload the gateway, change workers, drain and restore nodes, shut it down',
    gateway_publish: 'Validate, prepare, activate and abort runtime snapshots directly',
    config_read:
      'Read sites, upstreams, listeners, TLS profiles, the draft, its files and revisions',
    config_write: 'Change the draft: resources, files and revision notes',
    config_apply: 'Apply the draft, run dry runs and roll back',
    audit_read: 'Read and verify the audit trail',
    logs_read: "Search, follow and download the gateway's access and error logs",
    logs_delete: "Delete the gateway's logs of a site or of every site",
    alerts_read: 'Read alert rules, where they stand, their channels and the notifications sent',
    alerts_manage: 'Change alert rules and channels and send test notifications',
    host_read:
      "Read the host's figures and what its agent reports: the panel's directories, what holds ports and the gateway service",
    host_manage: 'Start, stop and restart the gateway service on the host',
    containers_read:
      'Read the container engines and their containers, images, networks, volumes and Compose projects',
    containers_inspect:
      "Read containers' logs and details and Compose files, which can hold secrets",
    containers_manage: 'Enable engines and start, stop, remove and prune what runs on them',
    platform_read:
      'Read the services of the control plane, the versions of what runs and whether an upgrade can start',
    platform_diagnose:
      'Download the diagnostic bundle: versions, health, recent failures and audit events, with secrets removed',
    identity_read: 'Read accounts, roles and sessions',
    identity_manage: 'Create, change, disable and unlock accounts, grant roles and end sessions',
    approval_manage:
      'Create, change and delete the policies that decide which changes need approval',
    approval_decide: 'Approve or reject changes other people asked to apply',
    approval_bypass:
      'Apply a change without its approvals in an emergency, giving a reason and an incident',
    certificate_read:
      'Read certificates, their names, validity and fingerprints, and check the hosts they cover',
    certificate_manage:
      'Upload, generate, replace and delete certificates; private keys are never returned',
    plugins_read:
      'Read plugins, their versions, grants, settings, limits and health, the trusted publisher keys and the names of kept secrets',
    plugins_manage:
      'Trust publisher keys, keep secrets for plugins, and grant, configure, limit, enable, disable, upgrade and roll back plugins',
  },
}

export default en
