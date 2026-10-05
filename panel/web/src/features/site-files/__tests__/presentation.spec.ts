import { describe, expect, it } from 'vitest'
import { childPath, crumbs, editable, normalizedPath, parentPath } from '../presentation'

describe('site file paths', () => {
  it('join, climb and break into crumbs', () => {
    expect(childPath('', 'shop')).toBe('shop')
    expect(childPath('shop', 'index.html')).toBe('shop/index.html')
    expect(parentPath('shop/assets/app.js')).toBe('shop/assets')
    expect(parentPath('shop')).toBe('')
    expect(crumbs('shop/assets')).toEqual([
      { name: 'shop', path: 'shop' },
      { name: 'assets', path: 'shop/assets' },
    ])
    expect(crumbs('')).toEqual([])
  })

  it('keep the address without stray slashes', () => {
    expect(normalizedPath('/shop/')).toBe('shop')
    expect(normalizedPath(['shop'])).toBe('')
    expect(normalizedPath(undefined)).toBe('')
  })

  it('edit text files up to 1 MiB', () => {
    expect(editable('index.html', 2048)).toBe(true)
    expect(editable('LICENSE', 2048)).toBe(true)
    expect(editable('logo.png', 2048)).toBe(false)
    expect(editable('bundle.js', 2 * 1024 * 1024)).toBe(false)
  })
})
