import { describe, expect, it } from 'vitest'
import type { RewriteRule } from '@/api/generated'
import { routeForm, routeInput } from '../forms'
import {
  describeRewrite,
  newRewrite,
  rewriteForm,
  rewriteProblem,
  targetProblem,
  toRewrite,
} from '../rewrites'

const rules: RewriteRule[] = [
  { kind: 'strip_prefix', prefix: '/api' },
  { kind: 'add_prefix', prefix: '/v2' },
  { kind: 'set_uri', template: '/index.php?q=$uri' },
  { kind: 'rewrite', pattern: '^/old/(.*)$', replacement: '/new/$1', flag: 'permanent' },
]

describe('rewrite forms', () => {
  it('round-trip every kind of rule', () => {
    for (const rule of rules) {
      expect(toRewrite(rewriteForm(rule))).toEqual(rule)
    }
    expect(rewriteForm({ kind: 'rewrite', pattern: 'a', replacement: '/b' }).flag).toBe('none')
  })

  it('reach the route input in their order, with the internal flag', () => {
    const form = routeForm(undefined, 10)
    form.rewrites = rules.map(rewriteForm)
    form.internal = true
    form.action.type = 'internal_redirect'
    form.action.target = ' @fallback '
    const input = routeInput(form)
    expect(input.rewrites).toEqual(rules)
    expect(input.internal).toBe(true)
    expect(input.action).toEqual({ type: 'internal_redirect', target: '@fallback' })
  })

  it('point out what keeps a rule from being saved', () => {
    expect(rewriteProblem(newRewrite('strip_prefix'))).toBe(
      'routes.rewrites.problems.prefixRequired',
    )
    for (const prefix of ['api', '/', '/api/']) {
      expect(rewriteProblem({ ...newRewrite('add_prefix'), prefix })).toBe(
        'routes.rewrites.problems.prefixShape',
      )
    }
    expect(rewriteProblem(newRewrite('set_uri'))).toBe('routes.rewrites.problems.templateRequired')
    expect(rewriteProblem({ ...newRewrite('rewrite'), pattern: '^/a' })).toBe(
      'routes.rewrites.problems.replacementRequired',
    )
    for (const rule of rules) {
      expect(rewriteProblem(rewriteForm(rule))).toBeUndefined()
    }
  })

  it('describe rules as the configuration language writes them', () => {
    expect(rules.map(describeRewrite)).toEqual([
      'strip_prefix /api',
      'add_prefix /v2',
      'set_uri /index.php?q=$uri',
      'rewrite ^/old/(.*)$ /new/$1 permanent',
    ])
  })

  it('accept paths and named routes as internal targets', () => {
    expect(targetProblem('/errors$uri')).toBeUndefined()
    expect(targetProblem('@fallback')).toBeUndefined()
    expect(targetProblem(' ')).toBe('sites.form.targetRequired')
    expect(targetProblem('@')).toBe('sites.form.targetShape')
    expect(targetProblem('errors')).toBe('sites.form.targetShape')
  })
})
