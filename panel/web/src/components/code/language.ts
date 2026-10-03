import { StreamLanguage, type StreamParser, type StringStream } from '@codemirror/language'

interface State {
  /** The next word starts a directive. */
  statement: boolean
  /** The quote of a string continuing on the next line. */
  quote: '"' | "'" | null
  depth: number
}

function string(stream: StringStream, state: State): string {
  while (!stream.eol()) {
    const next = stream.next()
    if (next === '\\') {
      stream.next()
    } else if (next === state.quote) {
      state.quote = null
      break
    }
  }
  return 'string'
}

function ends(character: string | undefined): boolean {
  return character === undefined || /[\s;{}]/.test(character)
}

/**
 * Tokens of the configuration language. Its lexical rules are NGINX's: a
 * directive name, arguments that are words or quoted strings, and `;` or a
 * block; `#` starts a comment where a token could start.
 */
export const configurationTokens: StreamParser<State> = {
  name: 'pingora-panel-conf',
  startState: () => ({ statement: true, quote: null, depth: 0 }),
  copyState: (state) => ({ ...state }),
  token(stream, state) {
    if (state.quote) {
      return string(stream, state)
    }
    if (stream.eatSpace()) {
      return null
    }
    const character = stream.peek()
    if (character === '#') {
      stream.skipToEnd()
      return 'comment'
    }
    if (character === '{' || character === '}' || character === ';') {
      stream.next()
      state.statement = true
      if (character === '{') {
        state.depth += 1
      } else if (character === '}') {
        state.depth = Math.max(0, state.depth - 1)
      }
      return character === ';' ? 'punctuation' : 'brace'
    }
    if (character === '"' || character === "'") {
      stream.next()
      state.quote = character
      state.statement = false
      return string(stream, state)
    }
    const directive = state.statement
    state.statement = false
    if (directive) {
      stream.match(/^[^\s;{}]+/)
      return 'keyword'
    }
    if (character === '$') {
      if (stream.match(/^\$\{[A-Za-z0-9_]*\}?/) || stream.match(/^\$[A-Za-z0-9_]*/)) {
        return 'variableName'
      }
    }
    if (stream.match(/^[a-z_]+=/)) {
      return 'attributeName'
    }
    while (!ends(stream.peek()) && stream.peek() !== '$') {
      if (stream.next() === '\\') {
        stream.next()
      }
    }
    const word = stream.current()
    if (/^(on|off)$/.test(word)) {
      return 'bool'
    }
    if (/^\d+(\.\d+)?([a-zA-Z]{1,2})?$/.test(word)) {
      return 'number'
    }
    return null
  },
  indent(state, textAfter, context) {
    const depth = state.depth - (/^\s*\}/.test(textAfter) ? 1 : 0)
    return Math.max(0, depth) * context.unit
  },
  languageData: {
    commentTokens: { line: '#' },
    closeBrackets: { brackets: ['{', '"', "'"] },
    indentOnInput: /^\s*\}$/,
  },
}

export const configurationLanguage = StreamLanguage.define(configurationTokens)
