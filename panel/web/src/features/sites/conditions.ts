import type { RouteCondition, ValueTest } from '@/api/generated'

export type ConditionKind = RouteCondition['kind']
export type TestOperator = ValueTest['op']

/** Conditions offered when adding one, groups last. */
export const CONDITION_KINDS: readonly ConditionKind[] = [
  'method',
  'host',
  'header',
  'query',
  'cookie',
  'client',
  'user_agent',
  'referer',
  'content_type',
  'any',
  'all',
  'not',
]

export const TEST_OPERATORS: readonly TestOperator[] = [
  'present',
  'absent',
  'equals',
  'prefix',
  'suffix',
  'contains',
  'regex',
]

export const GROUP_KINDS: readonly ConditionKind[] = ['any', 'all', 'not']
const NAMED_KINDS: readonly ConditionKind[] = ['header', 'query', 'cookie']
const TESTED_KINDS: readonly ConditionKind[] = [
  'header',
  'query',
  'cookie',
  'user_agent',
  'referer',
]
/** How deeply groups nest, as the gateway allows. */
export const MOST_DEPTH = 8

/** A condition as the form edits it. */
export interface ConditionForm {
  kind: ConditionKind
  /** Of a header, query parameter or cookie. */
  name: string
  op: TestOperator
  /** The value, or the regular expression. */
  value: string
  ignoreCase: boolean
  /** Methods, hosts, networks or media types, separated by commas. */
  list: string
  children: ConditionForm[]
}

export function isGroup(kind: ConditionKind): boolean {
  return GROUP_KINDS.includes(kind)
}

export function isNamed(kind: ConditionKind): boolean {
  return NAMED_KINDS.includes(kind)
}

export function isTested(kind: ConditionKind): boolean {
  return TESTED_KINDS.includes(kind)
}

/** Whether a test compares with a value. */
export function takesValue(op: TestOperator): boolean {
  return op !== 'present' && op !== 'absent'
}

export function newCondition(kind: ConditionKind): ConditionForm {
  return {
    kind,
    name: '',
    op: kind === 'header' || kind === 'query' || kind === 'cookie' ? 'equals' : 'contains',
    value: '',
    ignoreCase: false,
    list: kind === 'method' ? 'GET, HEAD' : '',
    children: [],
  }
}

function items(value: string): string[] {
  return value
    .split(',')
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
}

function testForm(test: ValueTest): Pick<ConditionForm, 'op' | 'value' | 'ignoreCase'> {
  switch (test.op) {
    case 'present':
    case 'absent':
      return { op: test.op, value: '', ignoreCase: false }
    case 'regex':
      return { op: 'regex', value: test.pattern, ignoreCase: test.ignore_case ?? false }
    default:
      return { op: test.op, value: test.value, ignoreCase: test.ignore_case ?? false }
  }
}

export function conditionForm(condition: RouteCondition): ConditionForm {
  const form = newCondition(condition.kind)
  switch (condition.kind) {
    case 'method':
      return { ...form, list: condition.methods.join(', ') }
    case 'host':
      return { ...form, list: condition.hosts.join(', ') }
    case 'client':
      return { ...form, list: condition.networks.join(', ') }
    case 'content_type':
      return { ...form, list: condition.types.join(', ') }
    case 'header':
    case 'query':
    case 'cookie':
      return { ...form, name: condition.name, ...testForm(condition.test) }
    case 'user_agent':
    case 'referer':
      return { ...form, ...testForm(condition.test) }
    case 'all':
    case 'any':
      return { ...form, children: condition.conditions.map(conditionForm) }
    case 'not': {
      const inner = condition.condition
      const children =
        inner.kind === 'all' && inner.conditions.length !== 1 ? inner.conditions : [inner]
      return { ...form, children: children.map(conditionForm) }
    }
  }
}

function toTest(form: ConditionForm): ValueTest {
  const ignore_case = form.ignoreCase || undefined
  switch (form.op) {
    case 'present':
    case 'absent':
      return { op: form.op }
    case 'regex':
      return { op: 'regex', pattern: form.value, ignore_case }
    default:
      return { op: form.op, value: form.value, ignore_case }
  }
}

export function toCondition(form: ConditionForm): RouteCondition {
  const name = form.name.trim()
  switch (form.kind) {
    case 'method':
      return { kind: 'method', methods: items(form.list) }
    case 'host':
      return { kind: 'host', hosts: items(form.list).map((host) => host.toLowerCase()) }
    case 'client':
      return { kind: 'client', networks: items(form.list) }
    case 'content_type':
      return { kind: 'content_type', types: items(form.list).map((type) => type.toLowerCase()) }
    case 'header':
    case 'query':
    case 'cookie':
      return { kind: form.kind, name, test: toTest(form) }
    case 'user_agent':
    case 'referer':
      return { kind: form.kind, test: toTest(form) }
    case 'all':
    case 'any':
      return { kind: form.kind, conditions: form.children.map(toCondition) }
    case 'not': {
      const children = form.children.map(toCondition)
      return {
        kind: 'not',
        condition: children.length === 1 ? children[0]! : { kind: 'all', conditions: children },
      }
    }
  }
}

/** Why a condition cannot be saved as written, if it cannot. */
export function conditionProblem(form: ConditionForm): string | undefined {
  if (isGroup(form.kind)) {
    if (form.children.length === 0) {
      return 'empty'
    }
    return form.children.map(conditionProblem).find(Boolean)
  }
  if (isNamed(form.kind) && !form.name.trim()) {
    return 'name'
  }
  if (isTested(form.kind)) {
    return takesValue(form.op) && form.op === 'regex' && !form.value ? 'value' : undefined
  }
  return items(form.list).length === 0 ? 'list' : undefined
}

/** A condition in a line, as the configuration language writes it. */
export function describeCondition(condition: RouteCondition): string {
  const test = (value: ValueTest): string => {
    const flag = (ignore?: boolean) => (ignore ? ' ignore_case' : '')
    switch (value.op) {
      case 'present':
      case 'absent':
        return value.op
      case 'regex':
        return `${value.ignore_case ? '~*' : '~'} ${value.pattern}`
      case 'equals':
        return `= ${value.value}${flag(value.ignore_case)}`
      case 'prefix':
        return `^= ${value.value}${flag(value.ignore_case)}`
      case 'suffix':
        return `$= ${value.value}${flag(value.ignore_case)}`
      case 'contains':
        return `*= ${value.value}${flag(value.ignore_case)}`
    }
  }
  switch (condition.kind) {
    case 'method':
      return `method ${condition.methods.join(' ')}`
    case 'host':
      return `host ${condition.hosts.join(' ')}`
    case 'client':
      return `client ${condition.networks.join(' ')}`
    case 'content_type':
      return `content_type ${condition.types.join(' ')}`
    case 'header':
    case 'query':
    case 'cookie':
      return `${condition.kind} ${condition.name} ${test(condition.test)}`
    case 'user_agent':
    case 'referer':
      return `${condition.kind} ${test(condition.test)}`
    case 'all':
    case 'any':
      return `${condition.kind} { ${condition.conditions.map(describeCondition).join('; ')} }`
    case 'not':
      return `not { ${describeCondition(condition.condition)} }`
  }
}

/** Header lines written one per line as `Name: value`; others are left out. */
export function headerLines(text: string): { name: string; value: string }[] {
  return text
    .split('\n')
    .map((line) => line.split(/:(.*)/s))
    .filter((parts) => parts.length > 1 && parts[0]!.trim())
    .map(([name, value]) => ({ name: name!.trim(), value: (value ?? '').trim() }))
}
