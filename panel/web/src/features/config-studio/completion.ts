import type { Completion, CompletionContext, CompletionResult } from '@codemirror/autocomplete'
import type { Context, DirectiveSpec } from '@/api/generated'

/** The file every configuration starts from. */
const ENTRY = 'main.conf'
/** Deeper include chains are not followed when finding a file's block. */
const MAX_INCLUDE_DEPTH = 8

interface Token {
  value: string
  offset: number
  punctuation?: ';' | '{' | '}'
}

/** Tokens as the language lexes them, without comments. */
function* tokens(text: string): Generator<Token> {
  let index = 0
  while (index < text.length) {
    const character = text[index]!
    if (/\s/.test(character)) {
      index += 1
    } else if (character === '#') {
      const end = text.indexOf('\n', index)
      index = end === -1 ? text.length : end
    } else if (character === ';' || character === '{' || character === '}') {
      yield { value: character, offset: index, punctuation: character }
      index += 1
    } else if (character === '"' || character === "'") {
      const start = index
      index += 1
      let value = ''
      while (index < text.length && text[index] !== character) {
        if (text[index] === '\\') {
          index += 1
        }
        value += text[index] ?? ''
        index += 1
      }
      index += 1
      yield { value, offset: start }
    } else {
      const start = index
      while (index < text.length && !/[\s;{}]/.test(text[index]!)) {
        index += text[index] === '\\' ? 2 : 1
      }
      yield { value: text.slice(start, index), offset: start }
    }
  }
}

/**
 * The block the end of `text` is in, following the blocks the schema says
 * each directive opens from `start`, the block the file is read in.
 */
export function contextAt(
  text: string,
  directives: readonly DirectiveSpec[],
  start: Context = 'main',
): Context {
  const stack: Context[] = [start]
  let statement: string | null = null
  for (const token of tokens(text)) {
    if (token.punctuation === '{') {
      const current = stack[stack.length - 1]!
      const opened = directives.find(
        (directive) => directive.name === statement && directive.contexts.includes(current),
      )?.block
      stack.push(opened ?? current)
      statement = null
    } else if (token.punctuation === '}') {
      if (stack.length > 1) {
        stack.pop()
      }
      statement = null
    } else if (token.punctuation === ';') {
      statement = null
    } else {
      statement ??= token.value
    }
  }
  return stack[stack.length - 1]!
}

/** Matches paths as an include in `from` does: relative to its directory, `*` within a segment. */
export function includeMatcher(from: string, pattern: string): RegExp {
  const directory = from.includes('/') ? from.slice(0, from.lastIndexOf('/') + 1) : ''
  const glob = directory + pattern
  let source = ''
  for (let index = 0; index < glob.length; index += 1) {
    if (glob.startsWith('**/', index)) {
      source += '(?:[^/]+/)*'
      index += 2
    } else if (glob.startsWith('**', index)) {
      source += '.*'
      index += 1
    } else if (glob[index] === '*') {
      source += '[^/]*'
    } else if (glob[index] === '?') {
      source += '[^/]'
    } else {
      source += glob[index]!.replace(/[.+^${}()|[\]\\]/g, '\\$&')
    }
  }
  return new RegExp(`^${source}$`)
}

/** The `include` directives of a file, with where each starts. */
function includes(text: string): { offset: number; patterns: string[] }[] {
  const found: { offset: number; patterns: string[] }[] = []
  let words: Token[] = []
  for (const token of tokens(text)) {
    if (!token.punctuation) {
      words.push(token)
      continue
    }
    if (token.punctuation === ';' && words[0]?.value === 'include') {
      found.push({ offset: words[0].offset, patterns: words.slice(1).map((word) => word.value) })
    }
    words = []
  }
  return found
}

/** The block a file's directives are read in: that of the include naming it. */
export function baseContext(
  path: string,
  files: Readonly<Record<string, string>>,
  directives: readonly DirectiveSpec[],
  depth = 0,
): Context {
  if (path === ENTRY || depth > MAX_INCLUDE_DEPTH) {
    return 'main'
  }
  for (const [from, text] of Object.entries(files)) {
    if (from === path) {
      continue
    }
    for (const include of includes(text)) {
      if (include.patterns.some((pattern) => includeMatcher(from, pattern).test(path))) {
        const start = baseContext(from, files, directives, depth + 1)
        return contextAt(text.slice(0, include.offset), directives, start)
      }
    }
  }
  return 'main'
}

function option(directive: DirectiveSpec): Completion {
  const deprecated = directive.deprecated
  return {
    label: directive.name,
    type: directive.block ? 'class' : 'keyword',
    detail: directive.syntax,
    info: deprecated
      ? `${directive.summary} (deprecated: ${deprecated.replacement})`
      : directive.summary,
    boost: deprecated ? -20 : 0,
    apply: `${directive.name} `,
  }
}

/** Completes directive names valid where the cursor is. */
export function directiveCompletion(
  directives: () => readonly DirectiveSpec[] | undefined,
  base: () => Context = () => 'main',
) {
  return (context: CompletionContext): CompletionResult | null => {
    const known = directives()
    const word = context.matchBefore(/[a-z_]*/)
    if (!known || !word || (word.from === word.to && !context.explicit)) {
      return null
    }
    const line = context.state.doc.lineAt(word.from)
    const before = context.state.sliceDoc(line.from, word.from)
    if (!/(^|[;{}])\s*$/.test(before)) {
      return null
    }
    const where = contextAt(context.state.sliceDoc(0, word.from), known, base())
    const options = new Map<string, Completion>()
    for (const directive of known) {
      if (directive.contexts.includes(where) && !options.has(directive.name)) {
        options.set(directive.name, option(directive))
      }
    }
    return { from: word.from, options: [...options.values()], validFor: /^[a-z_]*$/ }
  }
}
