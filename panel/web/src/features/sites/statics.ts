import type { DirectoryListing, StaticCacheRule } from '@/api/generated'
import { optionalNumber } from '@/lib/forms'

export const LISTINGS: readonly DirectoryListing[] = ['off', 'html', 'json']

export interface MediaTypeForm {
  extension: string
  type: string
}

export type CacheMode = 'max_age' | 'no_cache'
export type AgeUnit = 'seconds' | 'minutes' | 'hours' | 'days' | 'years'

export const AGE_UNITS: Record<AgeUnit, number> = {
  seconds: 1,
  minutes: 60,
  hours: 3_600,
  days: 86_400,
  years: 31_536_000,
}
const LARGEST_FIRST: AgeUnit[] = ['years', 'days', 'hours', 'minutes', 'seconds']
/** The longest max-age caches are bound to read (RFC 9111 §1.2.2). */
export const MOST_MAX_AGE_SECONDS = 2 ** 31

export interface CacheRuleForm {
  /** Comma-separated, such as `css, js`; empty for every file. */
  extensions: string
  mode: CacheMode
  age: number | string
  unit: AgeUnit
  immutable: boolean
}

export type CachePreset = 'assets' | 'html' | 'everything'
export const CACHE_PRESETS: readonly CachePreset[] = ['assets', 'html', 'everything']

export function newCacheRule(preset: CachePreset): CacheRuleForm {
  switch (preset) {
    case 'assets':
      return {
        extensions: 'css, js, mjs, woff2, svg, png, jpg, webp, avif',
        mode: 'max_age',
        age: 1,
        unit: 'years',
        immutable: true,
      }
    case 'html':
      return { extensions: 'html', mode: 'no_cache', age: '', unit: 'hours', immutable: false }
    case 'everything':
      return { extensions: '', mode: 'max_age', age: 1, unit: 'hours', immutable: false }
  }
}

export function cacheRuleForm(rule: StaticCacheRule): CacheRuleForm {
  const seconds = rule.max_age_seconds
  if (seconds === undefined || seconds === null) {
    return {
      extensions: (rule.extensions ?? []).join(', '),
      mode: 'no_cache',
      age: '',
      unit: 'hours',
      immutable: false,
    }
  }
  const unit = LARGEST_FIRST.find((candidate) => seconds % AGE_UNITS[candidate] === 0) ?? 'seconds'
  return {
    extensions: (rule.extensions ?? []).join(', '),
    mode: 'max_age',
    age: seconds / AGE_UNITS[unit],
    unit,
    immutable: rule.immutable ?? false,
  }
}

export function extensions(value: string): string[] {
  return [
    ...new Set(
      value
        .split(/[\s,]+/)
        .map((extension) => extension.trim().replace(/^\./, '').toLowerCase())
        .filter(Boolean),
    ),
  ]
}

export function toCacheRule(form: CacheRuleForm): StaticCacheRule {
  const rule: StaticCacheRule = { extensions: extensions(form.extensions) }
  if (form.mode === 'max_age') {
    rule.max_age_seconds = Math.round((optionalNumber(form.age) ?? 0) * AGE_UNITS[form.unit])
    rule.immutable = form.immutable
  }
  return rule
}

const EXTENSION = /^[a-z0-9+_-]{1,32}$/

/**
 * What keeps a cache rule from being saved, as a message key with values;
 * `earlier` are the rules before it, which take their extensions first.
 */
export function cacheRuleProblem(
  form: CacheRuleForm,
  earlier: readonly CacheRuleForm[] = [],
): { key: string; values?: Record<string, unknown> } | undefined {
  const own = extensions(form.extensions)
  const wrong = own.find((extension) => !EXTENSION.test(extension))
  if (wrong !== undefined) {
    return { key: 'sites.statics.problems.extension', values: { extension: wrong } }
  }
  const taken = new Set(earlier.flatMap((rule) => extensions(rule.extensions)))
  const repeated = own.find((extension) => taken.has(extension))
  if (repeated !== undefined) {
    return { key: 'sites.statics.problems.taken', values: { extension: repeated } }
  }
  if (own.length === 0 && earlier.some((rule) => extensions(rule.extensions).length === 0)) {
    return { key: 'sites.statics.problems.everyFileTwice' }
  }
  if (form.mode === 'max_age') {
    const seconds = (optionalNumber(form.age) ?? -1) * AGE_UNITS[form.unit]
    if (!Number.isInteger(seconds) || seconds < 0 || seconds > MOST_MAX_AGE_SECONDS) {
      return { key: 'sites.statics.problems.maxAge' }
    }
  }
  return undefined
}

export function mediaTypesForm(types?: Record<string, string> | null): MediaTypeForm[] {
  return Object.entries(types ?? {}).map(([extension, type]) => ({ extension, type }))
}

export function toMediaTypes(forms: readonly MediaTypeForm[]): Record<string, string> {
  return Object.fromEntries(
    forms.map((form) => [form.extension.trim().replace(/^\./, '').toLowerCase(), form.type.trim()]),
  )
}

/** A media type is `type/subtype`, maybe with parameters. */
export function isMediaType(value: string): boolean {
  return /^[!#$%&'*+.^_`|~0-9A-Za-z-]+\/[!#$%&'*+.^_`|~0-9A-Za-z-]+\s*(;.*)?$/.test(value.trim())
}

export function mediaTypeProblem(
  form: MediaTypeForm,
  earlier: readonly MediaTypeForm[] = [],
): { key: string; values?: Record<string, unknown> } | undefined {
  const extension = form.extension.trim().replace(/^\./, '').toLowerCase()
  if (!EXTENSION.test(extension)) {
    return { key: 'sites.statics.problems.extension', values: { extension: form.extension } }
  }
  if (
    earlier.some((other) => other.extension.trim().replace(/^\./, '').toLowerCase() === extension)
  ) {
    return { key: 'sites.statics.problems.mapped', values: { extension } }
  }
  return isMediaType(form.type) ? undefined : { key: 'sites.statics.problems.mediaType' }
}

/** Whether the static settings of an action keep it from being saved. */
export function staticProblem(settings: {
  mediaTypes: readonly MediaTypeForm[]
  defaultType: string
  cache: readonly CacheRuleForm[]
}): boolean {
  return (
    settings.mediaTypes.some((form, index) =>
      mediaTypeProblem(form, settings.mediaTypes.slice(0, index)),
    ) ||
    (settings.defaultType.trim() !== '' && !isMediaType(settings.defaultType)) ||
    settings.cache.some((form, index) => cacheRuleProblem(form, settings.cache.slice(0, index)))
  )
}

/** A rule as `Cache-Control` sends it, for summaries. */
export function describeCacheRule(rule: StaticCacheRule): string {
  const seconds = rule.max_age_seconds
  if (seconds === undefined || seconds === null) return 'no-cache'
  return rule.immutable ? `max-age=${seconds}, immutable` : `max-age=${seconds}`
}
