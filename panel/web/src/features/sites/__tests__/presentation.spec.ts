import { describe, expect, it } from 'vitest'
import { parseHosts } from '../presentation'

describe('parseHosts', () => {
  it('splits lines and commas and drops comments', () => {
    expect(parseHosts('a.example, b.example\n# note\n\n  c.example # inline\n')).toEqual([
      'a.example',
      'b.example',
      'c.example',
    ])
  })
})
