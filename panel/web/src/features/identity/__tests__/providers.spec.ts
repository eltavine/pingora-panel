import { describe, expect, it } from 'vitest'
import type { IdentityProviderResponse } from '@/api/generated'
import { callbackUrl, providerForm, providerInput, startUrl } from '../providers'

const corp: IdentityProviderResponse = {
  id: 'corp',
  display_name: 'Corporate',
  issuer: 'https://id.example',
  client_id: 'panel',
  has_client_secret: true,
  scopes: ['profile'],
  claims: { username: 'upn', display_name: 'name', email: 'mail', groups: 'groups' },
  group_roles: [{ group: 'ops', role: 'operator' }],
  create_accounts: true,
  enabled: true,
  created_at: '2026-10-03T00:00:00.000Z',
  updated_at: '2026-10-03T00:00:00.000Z',
}

describe('providerInput', () => {
  it('keeps the secret unless one is typed or the client becomes public', () => {
    const form = providerForm(corp)
    expect('client_secret' in providerInput(form, corp)).toBe(false)
    expect(providerInput({ ...form, clientSecret: 'new' }, corp).client_secret).toBe('new')
    expect(providerInput({ ...form, publicClient: true }, corp).client_secret).toBeNull()
  })

  it('splits scopes, drops empty mappings and keeps claims it does not edit', () => {
    const input = providerInput(
      {
        ...providerForm(corp),
        scopes: ' profile  email groups ',
        groupRoles: [
          { group: ' ops ', role: 'operator' },
          { group: '', role: 'viewer' },
        ],
      },
      corp,
    )
    expect(input.scopes).toEqual(['profile', 'email', 'groups'])
    expect(input.group_roles).toEqual([{ group: 'ops', role: 'operator' }])
    expect(input.claims).toEqual({
      username: 'upn',
      display_name: 'name',
      email: 'mail',
      groups: 'groups',
    })
  })

  it('starts new providers enabled, confidential and with the usual scopes', () => {
    const form = providerForm()
    expect([form.enabled, form.publicClient, form.scopes]).toEqual([true, false, 'profile email'])
  })
})

describe('provider URLs', () => {
  it('escape the provider and the return path', () => {
    expect(callbackUrl('https://panel.example', 'corp')).toBe(
      'https://panel.example/api/v1/auth/oidc/corp/callback',
    )
    expect(startUrl('corp', '/sites?q=a&b')).toBe(
      '/api/v1/auth/oidc/corp/start?return_to=%2Fsites%3Fq%3Da%26b',
    )
  })
})
