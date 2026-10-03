import type { IdentityProviderInput, IdentityProviderResponse } from '@/api/generated'

export interface GroupRoleRow {
  group: string
  role: string
}

export interface ProviderForm {
  id: string
  displayName: string
  issuer: string
  clientId: string
  /** A new secret; empty keeps the current one. */
  clientSecret: string
  publicClient: boolean
  scopes: string
  groupRoles: GroupRoleRow[]
  createAccounts: boolean
  enabled: boolean
  usernameClaim: string
  groupsClaim: string
}

const DEFAULT_SCOPES = 'profile email'

export function providerForm(provider?: IdentityProviderResponse): ProviderForm {
  return {
    id: provider?.id ?? '',
    displayName: provider?.display_name ?? '',
    issuer: provider?.issuer ?? '',
    clientId: provider?.client_id ?? '',
    clientSecret: '',
    publicClient: provider ? !provider.has_client_secret : false,
    scopes: provider ? provider.scopes.join(' ') : DEFAULT_SCOPES,
    groupRoles: (provider?.group_roles ?? []).map((mapping) => ({ ...mapping })),
    createAccounts: provider?.create_accounts ?? false,
    enabled: provider?.enabled ?? true,
    usernameClaim: provider?.claims.username ?? 'preferred_username',
    groupsClaim: provider?.claims.groups ?? 'groups',
  }
}

/** The request that saves `form`; the secret is sent only when it changes. */
export function providerInput(
  form: ProviderForm,
  provider?: IdentityProviderResponse,
): IdentityProviderInput {
  const input: IdentityProviderInput = {
    display_name: form.displayName.trim(),
    issuer: form.issuer.trim(),
    client_id: form.clientId.trim(),
    scopes: form.scopes.split(/\s+/).filter(Boolean),
    claims: {
      username: form.usernameClaim.trim() || 'preferred_username',
      display_name: provider?.claims.display_name ?? 'name',
      email: provider?.claims.email ?? 'email',
      groups: form.groupsClaim.trim() || 'groups',
    },
    group_roles: form.groupRoles
      .map((mapping) => ({ group: mapping.group.trim(), role: mapping.role }))
      .filter((mapping) => mapping.group && mapping.role),
    create_accounts: form.createAccounts,
    enabled: form.enabled,
  }
  if (form.publicClient) {
    input.client_secret = null
  } else if (form.clientSecret) {
    input.client_secret = form.clientSecret
  }
  return input
}

/** Where the provider sends people back; registered with the provider. */
export function callbackUrl(origin: string, id: string): string {
  return `${origin}/api/v1/auth/oidc/${encodeURIComponent(id)}/callback`
}

/** Starts signing in through a provider, returning to `next` afterwards. */
export function startUrl(id: string, next: string): string {
  return `/api/v1/auth/oidc/${encodeURIComponent(id)}/start?return_to=${encodeURIComponent(next)}`
}
