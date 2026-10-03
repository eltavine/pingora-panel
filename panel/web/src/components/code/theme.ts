import { HighlightStyle } from '@codemirror/language'
import { EditorView } from '@codemirror/view'
import { tags } from '@lezer/highlight'

const MONO =
  "'Geist Mono Variable', ui-monospace, 'SFMono-Regular', 'Cascadia Mono', Menlo, monospace"

/**
 * The console's monochrome tokens. The editor lives in a shadow root, where
 * the custom properties of the page still apply; severity is told by shape,
 * never by hue.
 */
export const editorTheme = EditorView.theme({
  '&': {
    color: 'var(--foreground)',
    backgroundColor: 'var(--background)',
    fontSize: '13px',
    height: '100%',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: MONO, lineHeight: '1.65' },
  '.cm-content': { caretColor: 'var(--foreground)', paddingBlock: '8px' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--foreground)' },
  '.cm-gutters': {
    backgroundColor: 'var(--muted)',
    color: 'var(--muted-foreground)',
    borderRight: '1px solid var(--border)',
  },
  '.cm-activeLine': { backgroundColor: 'color-mix(in oklab, var(--muted) 70%, transparent)' },
  '.cm-activeLineGutter': { backgroundColor: 'var(--accent)', color: 'var(--foreground)' },
  '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground':
    { backgroundColor: 'color-mix(in oklab, var(--foreground) 16%, transparent)' },
  '.cm-matchingBracket, &.cm-focused .cm-matchingBracket': {
    backgroundColor: 'transparent',
    outline: '1px solid var(--ring)',
  },
  '.cm-searchMatch': { outline: '1px solid var(--ring)', backgroundColor: 'transparent' },
  '.cm-searchMatch.cm-searchMatch-selected': {
    backgroundColor: 'color-mix(in oklab, var(--foreground) 16%, transparent)',
  },
  '.cm-panels': {
    backgroundColor: 'var(--muted)',
    color: 'var(--foreground)',
    borderColor: 'var(--border)',
  },
  '.cm-panel input, .cm-panel button, .cm-textfield, .cm-button': {
    fontFamily: 'inherit',
    fontSize: '12px',
    color: 'var(--foreground)',
    backgroundColor: 'var(--background)',
    backgroundImage: 'none',
    border: '1px solid var(--input)',
    borderRadius: 'calc(var(--radius) - 4px)',
  },
  '.cm-tooltip': {
    border: '1px solid var(--border)',
    backgroundColor: 'var(--popover)',
    color: 'var(--popover-foreground)',
    borderRadius: 'calc(var(--radius) - 2px)',
    boxShadow: '0 4px 16px color-mix(in oklab, var(--foreground) 10%, transparent)',
    overflow: 'hidden',
  },
  '.cm-tooltip-autocomplete > ul': { fontFamily: MONO, maxHeight: '16em' },
  '.cm-tooltip-autocomplete > ul > li': { padding: '2px 8px' },
  '.cm-tooltip-autocomplete > ul > li[aria-selected]': {
    backgroundColor: 'var(--accent)',
    color: 'var(--accent-foreground)',
  },
  '.cm-completionDetail': { color: 'var(--muted-foreground)', fontStyle: 'normal' },
  '.cm-completionInfo': { padding: '6px 8px', maxWidth: '28em' },
  '.cm-completionMatchedText': { textDecoration: 'none', fontWeight: '700' },
  '.cm-diagnostic': { padding: '4px 8px', borderLeft: '3px solid var(--foreground)' },
  '.cm-diagnostic-warning': { borderLeft: '3px dashed var(--foreground)' },
  '.cm-diagnostic-info': { borderLeft: '3px dotted var(--muted-foreground)' },
  '.cm-diagnosticSource': { color: 'var(--muted-foreground)' },
  '.cm-lintRange': { backgroundImage: 'none', paddingBottom: '0' },
  '.cm-lintRange-error': {
    textDecoration: 'underline wavy var(--foreground)',
    textUnderlineOffset: '3px',
  },
  '.cm-lintRange-warning': {
    textDecoration: 'underline dashed var(--foreground)',
    textUnderlineOffset: '3px',
  },
  '.cm-lintRange-info': {
    textDecoration: 'underline dotted var(--muted-foreground)',
    textUnderlineOffset: '3px',
  },
  '.cm-lint-marker': { width: '0.7em', height: '0.7em', margin: '0.45em 0 0 0.15em' },
  '.cm-lint-marker-error': {
    content: 'none',
    backgroundColor: 'var(--foreground)',
    borderRadius: '50%',
  },
  '.cm-lint-marker-warning': {
    content: 'none',
    border: '1.5px solid var(--foreground)',
    borderRadius: '50%',
    boxSizing: 'border-box',
  },
  '.cm-lint-marker-info': {
    content: 'none',
    border: '1.5px dotted var(--muted-foreground)',
    borderRadius: '50%',
    boxSizing: 'border-box',
  },
})

export const editorHighlight = HighlightStyle.define([
  { tag: tags.keyword, fontWeight: '600' },
  { tag: tags.comment, color: 'var(--muted-foreground)', fontStyle: 'italic' },
  { tag: tags.string, color: 'color-mix(in oklab, var(--foreground) 72%, var(--background))' },
  { tag: [tags.number, tags.bool], fontWeight: '500' },
  { tag: tags.variableName, textDecoration: 'underline dotted', textUnderlineOffset: '3px' },
  { tag: tags.attributeName, color: 'var(--muted-foreground)' },
  { tag: [tags.brace, tags.punctuation], color: 'var(--muted-foreground)' },
])
