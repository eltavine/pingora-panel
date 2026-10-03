<script setup lang="ts">
import { onBeforeUnmount, onMounted, useTemplateRef, watch } from 'vue'
import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from '@codemirror/autocomplete'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import {
  bracketMatching,
  indentOnInput,
  indentUnit,
  syntaxHighlighting,
} from '@codemirror/language'
import { forceLinting, lintKeymap } from '@codemirror/lint'
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search'
import { Compartment, EditorSelection, EditorState, type Extension } from '@codemirror/state'
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
} from '@codemirror/view'
import { configurationLanguage } from './language'
import { offsetOf, type Place } from './spans'
import { editorHighlight, editorTheme } from './theme'

const props = defineProps<{
  modelValue: string
  /** Identifies the document; each keeps its own selection and history. */
  path: string
  /** Accessible name of the text area. */
  label: string
  readonly?: boolean
  /** Fixed at mount: completion and lint sources read their inputs lazily. */
  extensions?: Extension[]
}>()

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const host = useTemplateRef<HTMLDivElement>('host')
const editable = new Compartment()
const states = new Map<string, EditorState>()
let view: EditorView | undefined

function access(readonly: boolean): Extension {
  return EditorState.readOnly.of(readonly)
}

function create(doc: string): EditorState {
  return EditorState.create({
    doc,
    extensions: [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      history(),
      drawSelection(),
      dropCursor(),
      indentOnInput(),
      bracketMatching(),
      closeBrackets(),
      autocompletion({ icons: false }),
      highlightActiveLine(),
      highlightSelectionMatches(),
      keymap.of([
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...searchKeymap,
        ...historyKeymap,
        ...completionKeymap,
        ...lintKeymap,
        indentWithTab,
      ]),
      indentUnit.of('    '),
      EditorState.tabSize.of(4),
      configurationLanguage,
      syntaxHighlighting(editorHighlight),
      editorTheme,
      editable.of(access(props.readonly ?? false)),
      EditorView.contentAttributes.of({ 'aria-label': props.label }),
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          emit('update:modelValue', update.state.doc.toString())
        }
      }),
      props.extensions ?? [],
    ],
  })
}

/** The smallest replacement turning `current` into `next`, so the cursor stays. */
function difference(current: string, next: string) {
  let from = 0
  const limit = Math.min(current.length, next.length)
  while (from < limit && current[from] === next[from]) {
    from += 1
  }
  let end = 0
  while (end < limit - from && current[current.length - 1 - end] === next[next.length - 1 - end]) {
    end += 1
  }
  return { from, to: current.length - end, insert: next.slice(from, next.length - end) }
}

onMounted(() => {
  const root = host.value!.attachShadow({ mode: 'open' })
  view = new EditorView({ state: create(props.modelValue), parent: root, root })
})

onBeforeUnmount(() => view?.destroy())

watch(
  () => [props.path, props.modelValue] as const,
  ([path, value], [previous]) => {
    if (!view) {
      return
    }
    if (path !== previous) {
      states.set(previous, view.state)
      const saved = states.get(path)
      view.setState(saved?.doc.toString() === value ? saved : create(value))
      return
    }
    const current = view.state.doc.toString()
    if (value !== current) {
      view.dispatch({ changes: difference(current, value) })
    }
  },
)

watch(
  () => props.readonly,
  (readonly) => view?.dispatch({ effects: editable.reconfigure(access(readonly ?? false)) }),
)

defineExpose({
  /** Selects a place and scrolls it into view. */
  reveal(start: Place, end?: Place) {
    if (!view) {
      return
    }
    const anchor = offsetOf(view.state.doc, start)
    const head = end ? offsetOf(view.state.doc, end) : anchor
    view.dispatch({
      selection: EditorSelection.single(anchor, head),
      effects: EditorView.scrollIntoView(anchor, { y: 'center' }),
    })
    view.focus()
  },
  /** Runs the lint sources now rather than after the next edit. */
  relint() {
    if (view) {
      forceLinting(view)
    }
  },
})
</script>

<template>
  <div ref="host" class="h-full min-h-0" />
</template>
