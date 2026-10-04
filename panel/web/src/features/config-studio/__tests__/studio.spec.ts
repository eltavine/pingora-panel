import { CompletionContext } from '@codemirror/autocomplete'
import { EditorState, Text } from '@codemirror/state'
import { describe, expect, it } from 'vitest'
import type { DiagnosticDetails, DirectiveSpec } from '@/api/generated'
import { baseContext, contextAt, directiveCompletion, includeMatcher } from '../completion'
import { editorDiagnostics } from '../lint'
import { isFilePath, sortPaths } from '@/lib/files'

function directive(
  name: string,
  contexts: DirectiveSpec['contexts'],
  block: DirectiveSpec['block'] = null,
): DirectiveSpec {
  return {
    name,
    contexts,
    block,
    min_args: 0,
    repeatable: true,
    summary: `${name} summary`,
    syntax: `${name} ...;`,
  }
}

const schema: DirectiveSpec[] = [
  directive('language_version', ['main']),
  directive('http', ['main'], 'http'),
  directive('upstream', ['http'], 'upstream'),
  directive('server', ['upstream']),
  directive('server', ['http'], 'server'),
  directive('server_name', ['server']),
  directive('proxy', ['server', 'route']),
  directive('route', ['server'], 'route'),
  directive('match', ['route']),
]

describe('contextAt', () => {
  it('follows the blocks directives open', () => {
    expect(contextAt('', schema)).toBe('main')
    expect(contextAt('http {\n', schema)).toBe('http')
    expect(contextAt('http {\n  upstream app {\n', schema)).toBe('upstream')
    expect(
      contextAt('http {\n  upstream app { server 10.0.0.1:80; }\n  server s {\n', schema),
    ).toBe('server')
    expect(contextAt('http { server s { route { match prefix /; }\n', schema)).toBe('server')
  })

  it('ignores braces in strings and comments', () => {
    expect(contextAt('http {\n  # }\n  server s {\n    respond 200 "}";\n', schema)).toBe('server')
  })
})

describe('included files', () => {
  it('match include patterns relative to the including file', () => {
    expect(includeMatcher('main.conf', 'sites/*.conf').test('sites/a.conf')).toBe(true)
    expect(includeMatcher('main.conf', 'sites/*.conf').test('sites/x/a.conf')).toBe(false)
    expect(includeMatcher('main.conf', 'sites/**/*.conf').test('sites/x/a.conf')).toBe(true)
    expect(includeMatcher('sites/a.conf', 'parts/?.conf').test('sites/parts/b.conf')).toBe(true)
    expect(includeMatcher('main.conf', 'a.conf').test('abconf')).toBe(false)
  })

  it('are read in the block of the include naming them', () => {
    const files = {
      'main.conf': 'language_version 1;\nhttp {\n  include "sites/*.conf";\n}\n',
      'sites/shop.conf': 'server shop {\n  include parts/routes.conf;\n}\n',
      'sites/parts/routes.conf': 'route { match prefix /; }\n',
      'orphan.conf': '',
    }
    expect(baseContext('main.conf', files, schema)).toBe('main')
    expect(baseContext('sites/shop.conf', files, schema)).toBe('http')
    expect(baseContext('sites/parts/routes.conf', files, schema)).toBe('server')
    expect(baseContext('orphan.conf', files, schema)).toBe('main')
  })
})

describe('directiveCompletion', () => {
  function complete(doc: string, explicit = false) {
    const state = EditorState.create({ doc })
    const source = directiveCompletion(() => schema)
    return source(new CompletionContext(state, doc.length, explicit))
  }

  it('offers the directives of the block at a statement start', () => {
    const result = complete('http {\n  server s {\n    pro')
    expect(result?.from).toBe(24)
    expect(result?.options.map((option) => option.label)).toEqual(['server_name', 'proxy', 'route'])
  })

  it('stays quiet inside arguments and on empty words unless asked', () => {
    expect(complete('http {\n  server s {\n    proxy ap')).toBeNull()
    expect(complete('http {\n  ')).toBeNull()
    expect(complete('http {\n  ', true)?.options.map((option) => option.label)).toEqual([
      'upstream',
      'server',
    ])
  })
})

describe('editorDiagnostics', () => {
  const doc = Text.of(['language_version 1;', 'http {', '    server s { proxy nowhere; }', '}'])
  const diagnostics: DiagnosticDetails[] = [
    {
      code: 'DSL_REFERENCE',
      severity: 'ERROR',
      message: 'no upstream is named "nowhere"',
      source_span: 'main.conf:3.22-28',
      help: 'define it with upstream nowhere { ... }',
    },
    { code: 'DSL_QUOTES', severity: 'WARNING', message: 'quotes', source_span: 'other.conf:1.1' },
    { code: 'VALIDATION', severity: 'ERROR', message: 'no place', resource_id: 'sites/1' },
  ]

  it('keeps diagnostics of the open file as ranges', () => {
    const [only, ...rest] = editorDiagnostics(diagnostics, 'main.conf', doc)
    expect(rest).toEqual([])
    expect(doc.sliceString(only!.from, only!.to)).toBe('nowhere')
    expect(only).toMatchObject({
      severity: 'error',
      source: 'DSL_REFERENCE',
      message: 'no upstream is named "nowhere"\ndefine it with upstream nowhere { ... }',
    })
  })
})

describe('files', () => {
  it('accepts relative .conf paths only', () => {
    const valid = ['main.conf', 'sites/blog.conf', 'a_b/c-d.v2.conf']
    const invalid = ['', 'main', '/etc/x.conf', '../x.conf', 'a/../x.conf', 'a//b.conf', '.x.conf']
    expect(valid.filter(isFilePath)).toEqual(valid)
    expect(invalid.filter(isFilePath)).toEqual([])
  })

  it('lists main.conf first', () => {
    expect(sortPaths(['sites/b.conf', 'main.conf', 'a.conf'])).toEqual([
      'main.conf',
      'a.conf',
      'sites/b.conf',
    ])
  })
})
