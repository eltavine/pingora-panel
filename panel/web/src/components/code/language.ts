import { StreamLanguage, type StreamParser, type StringStream } from '@codemirror/language'
import { lua } from '@codemirror/legacy-modes/mode/lua'

/** The code of a `*_by_lua_block`, read by Lua's own tokens. */
interface LuaBlock {
  state: unknown
  /** Braces open in the code, the block's own included. */
  braces: number
}

interface State {
  /** The next word starts a directive. */
  statement: boolean
  /** The quote of a string continuing on the next line. */
  quote: '"' | "'" | null
  depth: number
  /** The directive just named takes a Lua block. */
  luaNext: boolean
  lua: LuaBlock | null
}

/** Lua's token in a block; the brace that closes the block ends it. */
function luaToken(stream: StringStream, state: State, block: LuaBlock): string | null {
  const style = lua.token(stream, block.state)
  const text = stream.current()
  if (text === '{') {
    block.braces += 1
  } else if (text === '}') {
    block.braces -= 1
    if (block.braces === 0) {
      state.lua = null
      state.depth = Math.max(0, state.depth - 1)
      state.statement = true
      return 'brace'
    }
  }
  return style
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
 * block; `#` starts a comment where a token could start. The block of a
 * `*_by_lua_block` directive is Lua, read with Lua's tokens as the language
 * reads it.
 */
export const configurationTokens: StreamParser<State> = {
  name: 'pingora-panel-conf',
  startState: () => ({ statement: true, quote: null, depth: 0, luaNext: false, lua: null }),
  copyState: (state) => ({
    ...state,
    lua: state.lua && { braces: state.lua.braces, state: { ...(state.lua.state as object) } },
  }),
  token(stream, state) {
    if (state.lua) {
      return luaToken(stream, state, state.lua)
    }
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
      if (character === '{' && state.luaNext) {
        state.luaNext = false
        state.depth += 1
        state.lua = { state: lua.startState!(4), braces: 1 }
        return 'brace'
      }
      state.luaNext = false
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
      state.luaNext = stream.current().endsWith('_by_lua_block')
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
    if (state.lua) {
      const inner = lua.indent!(state.lua.state, textAfter, context) ?? 0
      return Math.max(0, state.depth * context.unit + inner)
    }
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

/** The configuration's `.lua` files. */
export const luaLanguage = StreamLanguage.define(lua)

/** The language a document is written in: Lua for `.lua` files or when said so. */
export function languageOf(path: string, lua = false) {
  return lua || path.endsWith('.lua') ? luaLanguage : configurationLanguage
}
