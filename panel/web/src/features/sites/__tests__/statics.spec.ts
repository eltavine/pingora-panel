import { describe, expect, it } from 'vitest'
import type { Action, StaticCacheRule } from '@/api/generated'
import { actionForm, toAction } from '../forms'
import {
  cacheRuleForm,
  cacheRuleProblem,
  describeCacheRule,
  isMediaType,
  mediaTypeProblem,
  newCacheRule,
  staticProblem,
  toCacheRule,
} from '../statics'

const rules: StaticCacheRule[] = [
  { extensions: ['css', 'js'], max_age_seconds: 31_536_000, immutable: true },
  { extensions: ['html'] },
  { extensions: [], max_age_seconds: 5400, immutable: false },
]

describe('cache rule forms', () => {
  it('round-trip rules in the largest unit that divides them', () => {
    for (const rule of rules) {
      expect(toCacheRule(cacheRuleForm(rule))).toEqual(rule)
    }
    expect(cacheRuleForm(rules[0]!)).toMatchObject({ age: 1, unit: 'years' })
    expect(cacheRuleForm(rules[2]!)).toMatchObject({ age: 90, unit: 'minutes' })
    expect(toCacheRule({ ...newCacheRule('html'), extensions: ' .HTML, htm ' })).toEqual({
      extensions: ['html', 'htm'],
    })
    expect(rules.map(describeCacheRule)).toEqual([
      'max-age=31536000, immutable',
      'no-cache',
      'max-age=5400',
    ])
  })

  it('point out rules that never apply or say too much', () => {
    const assets = newCacheRule('assets')
    const everything = newCacheRule('everything')
    expect(cacheRuleProblem({ ...assets, extensions: 'c$s' })?.key).toBe(
      'sites.statics.problems.extension',
    )
    expect(cacheRuleProblem({ ...everything, extensions: 'css' }, [assets])).toEqual({
      key: 'sites.statics.problems.taken',
      values: { extension: 'css' },
    })
    expect(cacheRuleProblem(everything, [everything])?.key).toBe(
      'sites.statics.problems.everyFileTwice',
    )
    expect(cacheRuleProblem({ ...everything, age: 100, unit: 'years' })?.key).toBe(
      'sites.statics.problems.maxAge',
    )
    expect(cacheRuleProblem({ ...everything, age: 0.5, unit: 'seconds' })?.key).toBe(
      'sites.statics.problems.maxAge',
    )
    expect(cacheRuleProblem(assets)).toBeUndefined()
  })
})

describe('media type forms', () => {
  it('want extensions once each and type/subtype', () => {
    expect(isMediaType('application/manifest+json')).toBe(true)
    expect(isMediaType('text/plain; charset=utf-8')).toBe(true)
    expect(isMediaType('image')).toBe(false)
    expect(mediaTypeProblem({ extension: '.WASM', type: 'application/wasm' })).toBeUndefined()
    expect(
      mediaTypeProblem({ extension: 'wasm', type: 'application/wasm' }, [
        { extension: 'WASM', type: 'application/x' },
      ]),
    ).toEqual({ key: 'sites.statics.problems.mapped', values: { extension: 'wasm' } })
    expect(mediaTypeProblem({ extension: 'wasm', type: 'wasm' })?.key).toBe(
      'sites.statics.problems.mediaType',
    )
  })
})

describe('static actions', () => {
  it('round-trip listings, media types, default types and cache rules', () => {
    const action: Action = {
      type: 'static',
      root: 'files',
      index_files: ['index.html'],
      spa_fallback: false,
      listing: 'json',
      media_types: { wasm: 'application/wasm' },
      default_type: 'text/plain',
      cache: rules,
    }
    const form = actionForm(action)
    expect(staticProblem(form)).toBe(false)
    expect(toAction(form)).toEqual(action)
    form.defaultType = 'binary'
    expect(staticProblem(form)).toBe(true)
    const plain = actionForm({ type: 'static', root: 'shop' })
    expect(toAction(plain)).toEqual({
      type: 'static',
      root: 'shop',
      index_files: [],
      spa_fallback: false,
    })
  })
})
