import { describe, expect, it } from 'vitest'
import type { LuaScriptInfo } from '@/api/generated'
import {
  duration,
  handlerCount,
  isFile,
  outcomeTone,
  parseHeaders,
  shortVersion,
} from '../presentation'

const script = (id: string, file: string, uses = 0): LuaScriptInfo => ({
  id,
  file,
  line: 1,
  sha256: 'ab'.repeat(32),
  bytes: 10,
  lines: 1,
  code: 'return',
  uses: Array.from({ length: uses }, () => ({ resource: 'lua', label: 'http', phase: 'access' })),
  requires: [],
})

describe('Lua presentation', () => {
  it('tells files from code written in blocks', () => {
    expect(isFile(script('lua/auth.lua', 'lua/auth.lua'))).toBe(true)
    expect(isFile(script('main.conf:12', 'main.conf'))).toBe(false)
    expect(handlerCount([script('a', 'a', 2), script('b', 'b', 1)])).toBe(3)
    expect(shortVersion('0123456789abcdef')).toBe('0123456789ab')
  })

  it('reads header lines and reports the ones without a colon', () => {
    expect(parseHeaders('X-Key: k\n\nbroken\nAccept:  text/html ')).toEqual({
      headers: [
        { name: 'X-Key', value: 'k' },
        { name: 'Accept', value: 'text/html' },
      ],
      invalid: [3],
    })
  })

  it('gives every outcome a tone and durations a unit', () => {
    expect(outcomeTone('failed')).toBe('negative')
    expect(outcomeTone('respond')).toBe('positive')
    expect(outcomeTone('continue')).toBe('neutral')
    expect(duration(420)).toBe('420 µs')
    expect(duration(1500)).toBe('1.50 ms')
    expect(duration(25_000)).toBe('25.0 ms')
  })
})
