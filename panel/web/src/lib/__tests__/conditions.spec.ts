import { describe, expect, it } from 'vitest'
import type { RouteCondition } from '@/api/generated'
import {
  conditionForm,
  conditionProblem,
  describeCondition,
  headerLines,
  newCondition,
  toCondition,
} from '../conditions'

const conditions: RouteCondition[] = [
  { kind: 'method', methods: ['GET', 'HEAD'] },
  { kind: 'header', name: 'x-env', test: { op: 'equals', value: 'staging', ignore_case: true } },
  { kind: 'query', name: 'debug', test: { op: 'absent' } },
  { kind: 'user_agent', test: { op: 'regex', pattern: 'bot' } },
  { kind: 'client', networks: ['10.0.0.0/8', '2001:db8::1'] },
  {
    kind: 'any',
    conditions: [
      { kind: 'cookie', name: 'canary', test: { op: 'present' } },
      { kind: 'content_type', types: ['application/json'] },
    ],
  },
  {
    kind: 'not',
    condition: {
      kind: 'all',
      conditions: [
        { kind: 'client', networks: ['192.0.2.0/24'] },
        { kind: 'referer', test: { op: 'prefix', value: 'https://shop.example/' } },
      ],
    },
  },
  { kind: 'not', condition: { kind: 'host', hosts: ['old.shop.example'] } },
]

describe('route conditions', () => {
  it('round-trip through the form', () => {
    expect(conditions.map(conditionForm).map(toCondition)).toEqual(conditions)
  })

  it('are described as the configuration language writes them', () => {
    expect(conditions.map(describeCondition)).toEqual([
      'method GET HEAD',
      'header x-env = staging ignore_case',
      'query debug absent',
      'user_agent ~ bot',
      'client 10.0.0.0/8 2001:db8::1',
      'any { cookie canary present; content_type application/json }',
      'not { all { client 192.0.2.0/24; referer ^= https://shop.example/ } }',
      'not { host old.shop.example }',
    ])
  })

  it('name what keeps them from being saved', () => {
    expect(conditionProblem(newCondition('header'))).toBe('name')
    expect(conditionProblem({ ...newCondition('client'), list: ' , ' })).toBe('list')
    expect(conditionProblem(newCondition('any'))).toBe('empty')
    expect(conditionProblem(newCondition('method'))).toBeUndefined()
  })

  it('read header lines and leave out the rest', () => {
    expect(headerLines('X-Env: staging\nbroken\n\nAccept: a: b')).toEqual([
      { name: 'X-Env', value: 'staging' },
      { name: 'Accept', value: 'a: b' },
    ])
  })
})
