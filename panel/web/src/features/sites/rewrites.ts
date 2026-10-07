import type { RewriteFlag, RewriteRule } from '@/api/generated'

export type RewriteKind = RewriteRule['kind']

export const REWRITE_KINDS: readonly RewriteKind[] = [
  'strip_prefix',
  'add_prefix',
  'set_uri',
  'rewrite',
]
export const REWRITE_FLAGS: readonly RewriteFlag[] = [
  'none',
  'last',
  'break',
  'redirect',
  'permanent',
]

/** Every rule kind's fields at once, so switching kind keeps typed input. */
export interface RewriteForm {
  kind: RewriteKind
  prefix: string
  template: string
  pattern: string
  replacement: string
  flag: RewriteFlag
}

export function newRewrite(kind: RewriteKind): RewriteForm {
  return { kind, prefix: '', template: '', pattern: '', replacement: '', flag: 'none' }
}

export function rewriteForm(rule: RewriteRule): RewriteForm {
  const form = newRewrite(rule.kind)
  switch (rule.kind) {
    case 'strip_prefix':
    case 'add_prefix':
      form.prefix = rule.prefix
      break
    case 'set_uri':
      form.template = rule.template
      break
    case 'rewrite':
      form.pattern = rule.pattern
      form.replacement = rule.replacement
      form.flag = rule.flag ?? 'none'
      break
  }
  return form
}

export function toRewrite(form: RewriteForm): RewriteRule {
  switch (form.kind) {
    case 'strip_prefix':
    case 'add_prefix':
      return { kind: form.kind, prefix: form.prefix.trim() }
    case 'set_uri':
      return { kind: 'set_uri', template: form.template.trim() }
    case 'rewrite':
      return {
        kind: 'rewrite',
        pattern: form.pattern,
        replacement: form.replacement.trim(),
        flag: form.flag,
      }
  }
}

/**
 * What keeps a rule from being saved, as a message key; the gateway checks
 * patterns and templates themselves.
 */
export function rewriteProblem(form: RewriteForm): string | undefined {
  switch (form.kind) {
    case 'strip_prefix':
    case 'add_prefix': {
      const prefix = form.prefix.trim()
      if (prefix === '') return 'routes.rewrites.problems.prefixRequired'
      if (!prefix.startsWith('/') || prefix === '/' || prefix.endsWith('/')) {
        return 'routes.rewrites.problems.prefixShape'
      }
      return undefined
    }
    case 'set_uri':
      return form.template.trim() === '' ? 'routes.rewrites.problems.templateRequired' : undefined
    case 'rewrite':
      if (form.pattern === '') return 'routes.rewrites.problems.patternRequired'
      return form.replacement.trim() === ''
        ? 'routes.rewrites.problems.replacementRequired'
        : undefined
  }
}

/** A rule as the configuration language writes it, for summaries. */
export function describeRewrite(rule: RewriteRule): string {
  switch (rule.kind) {
    case 'strip_prefix':
    case 'add_prefix':
      return `${rule.kind} ${rule.prefix}`
    case 'set_uri':
      return `set_uri ${rule.template}`
    case 'rewrite':
      return [
        'rewrite',
        rule.pattern,
        rule.replacement,
        rule.flag && rule.flag !== 'none' ? rule.flag : '',
      ]
        .filter(Boolean)
        .join(' ')
  }
}

/** Whether an internal redirect target is a path or a named route. */
export function targetProblem(target: string): string | undefined {
  const value = target.trim()
  if (value === '') return 'sites.form.targetRequired'
  if (value === '@') return 'sites.form.targetShape'
  return value.startsWith('/') || value.startsWith('@') || value.startsWith('$')
    ? undefined
    : 'sites.form.targetShape'
}
