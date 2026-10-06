import { describe, expect, it } from 'vitest'
import { Database, Package, PackageCheck } from '@lucide/vue'
import type { LuaModules, LuaScriptInfo } from '@/api/generated'
import {
  duration,
  handlerCount,
  isFile,
  libraryIcon,
  moduleGroups,
  moduleOrigin,
  ORIGIN_ICONS,
  outcomeTone,
  parseHeaders,
  shortVersion,
} from '../presentation'

const script = (id: string, file: string, uses = 0, requires: string[] = []): LuaScriptInfo => ({
  id,
  file,
  line: 1,
  sha256: 'ab'.repeat(32),
  bytes: 10,
  lines: 1,
  code: 'return',
  uses: Array.from({ length: uses }, () => ({ resource: 'lua', label: 'http', phase: 'access' })),
  requires,
})

const modules: LuaModules = {
  built_in: [
    { name: 'cjson', library: 'lua-cjson' },
    { name: 'resty.redis', library: 'lua-resty-redis' },
    { name: 'resty.limit.req', library: 'lua-resty-limit-traffic' },
    { name: 'resty.limit.conn', library: 'lua-resty-limit-traffic' },
  ],
  refused: [{ name: 'ffi', reason: 'native code would run outside the sandbox' }],
}

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

  it('groups built-in modules by library, the libraries scripts load from first', () => {
    const scripts = [script('lua/a.lua', 'lua/a.lua', 1, ['resty.limit.req', 'resty.redis'])]
    expect(moduleGroups(modules.built_in, scripts)).toEqual([
      {
        library: 'lua-resty-limit-traffic',
        modules: ['resty.limit.req', 'resty.limit.conn'],
        loaded: 1,
      },
      { library: 'lua-resty-redis', modules: ['resty.redis'], loaded: 1 },
      { library: 'lua-cjson', modules: ['cjson'], loaded: 0 },
    ])
    expect(libraryIcon('lua-resty-redis')).toBe(Database)
    expect(libraryIcon('unheard-of')).toBe(Package)
  })

  it('tells where require finds each module', () => {
    const scripts = [{ ...script('lua/auth.lua', 'lua/auth.lua'), module: 'auth' }]
    expect(moduleOrigin('auth', modules, scripts)).toBe('file')
    expect(moduleOrigin('resty.redis', modules, scripts)).toBe('built_in')
    expect(moduleOrigin('ffi', modules, scripts)).toBe('refused')
    expect(moduleOrigin('resty.openidc', modules, scripts)).toBe('missing')
    expect(moduleOrigin('resty.redis', undefined, scripts)).toBeUndefined()
    expect(ORIGIN_ICONS.built_in).toBe(PackageCheck)
  })
})
