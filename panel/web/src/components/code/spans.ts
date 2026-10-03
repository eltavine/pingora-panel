import type { Text } from '@codemirror/state'

/** A position in a file, 1-based, with columns counted in characters. */
export interface Place {
  line: number
  column: number
}

/** A source range as the API reports it, with an inclusive end. */
export interface SourceSpan {
  file: string
  start: Place
  end: Place
}

const SPAN = /^(.+):(\d+)\.(\d+)(?:-(?:(\d+)\.)?(\d+))?$/

/**
 * Reads the GNU format the API uses: `file:line.column`,
 * `file:line.column-column` or `file:line.column-line.column`.
 */
export function parseSpan(value: string | null | undefined): SourceSpan | undefined {
  const match = value ? SPAN.exec(value) : null
  if (!match) {
    return undefined
  }
  const [, file, line, column, endLine, endColumn] = match
  const start = { line: Number(line), column: Number(column) }
  return {
    file: file!,
    start,
    end: {
      line: endLine ? Number(endLine) : start.line,
      column: endColumn ? Number(endColumn) : start.column,
    },
  }
}

/** The document offset of a place; characters outside the BMP count once. */
export function offsetOf(doc: Text, place: Place): number {
  const line = doc.line(Math.min(Math.max(place.line, 1), doc.lines))
  let offset = line.from
  let column = 1
  for (const character of line.text) {
    if (column >= place.column) {
      break
    }
    offset += character.length
    column += 1
  }
  return offset
}

/** The document range a span covers, its last character included. */
export function rangeOf(doc: Text, span: SourceSpan): { from: number; to: number } {
  const from = offsetOf(doc, span.start)
  const last = offsetOf(doc, span.end)
  const code = doc.sliceString(last, last + 2).codePointAt(0)
  const to =
    code === undefined || last >= doc.lineAt(last).to ? last : last + (code > 0xffff ? 2 : 1)
  return { from, to: Math.max(to, from) }
}
