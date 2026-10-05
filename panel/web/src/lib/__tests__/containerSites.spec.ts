import { describe, expect, it } from 'vitest'
import type { ContainerEndpointView, SiteLinkView } from '@/api/generated'
import { defaultEndpoint, endpointAddress, linkedSites, linksByContainer } from '../containerSites'

const published: ContainerEndpointView = {
  host: '127.0.0.1',
  port: 8081,
  container_port: 80,
  route: 'published',
  network: null,
}
const network: ContainerEndpointView = {
  host: '172.18.0.2',
  port: 9000,
  container_port: 9000,
  route: 'network',
  network: 'shop_default',
}

function link(container: string, site: string, upstream: string): SiteLinkView {
  return {
    container_id: container,
    container,
    site_id: site,
    site,
    upstream_id: upstream,
    upstream,
    node: '127.0.0.1:8081',
    route: 'published',
  }
}

describe('container sites', () => {
  it('write endpoints as upstream nodes name them', () => {
    expect(endpointAddress(published)).toBe('127.0.0.1:8081')
    expect(endpointAddress({ host: 'fd00::2', port: 80 })).toBe('[fd00::2]:80')
  })

  it('proxy to the declared port, or else the first endpoint', () => {
    expect(defaultEndpoint({ endpoints: [published, network] })).toBe(published)
    expect(
      defaultEndpoint({
        endpoints: [published, network],
        declared_site: { name: null, domains: ['shop.example'], port: 9000 },
      }),
    ).toBe(network)
    expect(
      defaultEndpoint({
        endpoints: [published],
        declared_site: { name: null, domains: ['shop.example'], port: 443 },
      }),
    ).toBe(published)
    expect(defaultEndpoint({ endpoints: [] })).toBeUndefined()
  })

  it('group links by container and name each site once', () => {
    const links = [link('b2', 'shop', 'web'), link('b2', 'shop', 'api'), link('c3', 'blog', 'blog')]
    const grouped = linksByContainer(links)
    expect(grouped.get('b2')).toHaveLength(2)
    expect(linkedSites(grouped.get('b2') ?? [])).toEqual([{ id: 'shop', name: 'shop' }])
    expect(grouped.get('a1')).toBeUndefined()
  })
})
