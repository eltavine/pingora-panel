import { StringStream } from '@codemirror/language'
import { Text } from '@codemirror/state'
import { describe, expect, it } from 'vitest'
import { configurationTokens } from '../language'
import { offsetOf, parseSpan, placeOf, rangeOf } from '../spans'

describe('parseSpan', () => {
  it('reads each GNU form with an inclusive end', () => {
    expect(parseSpan('main.conf:3.22-28')).toEqual({
      file: 'main.conf',
      start: { line: 3, column: 22 },
      end: { line: 3, column: 28 },
    })
    expect(parseSpan('sites/a.conf:2.5-4.1')?.end).toEqual({ line: 4, column: 1 })
    expect(parseSpan('main.conf:7.3')?.end).toEqual({ line: 7, column: 3 })
  })

  it('rejects other text', () => {
    for (const value of [undefined, null, '', 'main.conf', 'main.conf:3', 'sites/7']) {
      expect(parseSpan(value)).toBeUndefined()
    }
  })
})

describe('offsets', () => {
  const doc = Text.of(['language_version 1;', 'server 😀 { proxy app; }'])

  it('counts columns in characters', () => {
    expect(offsetOf(doc, { line: 2, column: 1 })).toBe(20)
    // The emoji is one column but two UTF-16 units.
    expect(offsetOf(doc, { line: 2, column: 10 })).toBe(20 + 10)
  })

  it('covers the last character of a span', () => {
    const span = parseSpan('main.conf:2.8')!
    expect(rangeOf(doc, span)).toEqual({ from: 27, to: 29 })
    const word = parseSpan('main.conf:2.12-16')!
    expect(doc.sliceString(rangeOf(doc, word).from, rangeOf(doc, word).to)).toBe('proxy')
  })

  it('clamps places outside the document', () => {
    expect(offsetOf(doc, { line: 9, column: 99 })).toBe(doc.length)
  })

  it('turns offsets back into places', () => {
    expect(placeOf(doc, 0)).toEqual({ line: 1, column: 1 })
    expect(placeOf(doc, 20 + 10)).toEqual({ line: 2, column: 10 })
    for (const column of [1, 7, 8, 9, 12, 24]) {
      expect(placeOf(doc, offsetOf(doc, { line: 2, column }))).toEqual({ line: 2, column })
    }
  })
})

function tokens(lines: string[]): [string, string | null][] {
  const parser = configurationTokens
  const state = parser.startState!(4)
  const result: [string, string | null][] = []
  for (const line of lines) {
    const stream = new StringStream(line, 4, 4)
    while (!stream.eol()) {
      const style = parser.token(stream, state)
      if (stream.current().trim()) {
        result.push([stream.current(), style])
      }
      stream.start = stream.pos
    }
  }
  return result
}

describe('configurationTokens', () => {
  it('highlights directives, parameters, variables and comments', () => {
    expect(
      tokens([
        'server shop { # storefront',
        '    respond 503 "body=be back" retry_after=30s;',
        '    set $tag v$host;',
        '}',
      ]),
    ).toEqual([
      ['server', 'keyword'],
      ['shop', null],
      ['{', 'brace'],
      ['# storefront', 'comment'],
      ['respond', 'keyword'],
      ['503', 'number'],
      ['"body=be back"', 'string'],
      ['retry_after=', 'attributeName'],
      ['30s', 'number'],
      [';', 'punctuation'],
      ['set', 'keyword'],
      ['$tag', 'variableName'],
      ['v', null],
      ['$host', 'variableName'],
      [';', 'punctuation'],
      ['}', 'brace'],
    ])
  })

  it('keeps a hash inside a word and strings across lines', () => {
    expect(tokens(['return 302 /a#b;'])).toContainEqual(['/a#b', null])
    expect(tokens(['respond 200 "one', 'two";'])).toEqual([
      ['respond', 'keyword'],
      ['200', 'number'],
      ['"one', 'string'],
      ['two"', 'string'],
      [';', 'punctuation'],
    ])
  })
})

describe('Lua blocks', () => {
  it('read their code with Lua tokens up to the brace that closes them', () => {
    const read = tokens([
      'access_by_lua_block {',
      '    local t = { "}", [[}]] } -- }',
      '    if ngx.var.arg_x then return ngx.exit(403) end',
      '}',
      'listen 80;',
    ])
    expect(read[0]).toEqual(['access_by_lua_block', 'keyword'])
    expect(read).toContainEqual(['local', 'keyword'])
    expect(read).toContainEqual(['"}"', 'string'])
    expect(read).toContainEqual(['[[}]]', 'string'])
    expect(read).toContainEqual(['-- }', 'comment'])
    expect(read).toContainEqual(['return', 'keyword'])
    const closing = read.findIndex(([text]) => text === 'listen')
    expect(read[closing - 1]).toEqual(['}', 'brace'])
    expect(read[closing]).toEqual(['listen', 'keyword'])
  })

  it('leave arguments named like blocks alone', () => {
    const read = tokens(['set $x content_by_lua_block;', 'server s { }'])
    expect(read).toContainEqual(['server', 'keyword'])
    expect(read).toContainEqual(['{', 'brace'])
  })
})
