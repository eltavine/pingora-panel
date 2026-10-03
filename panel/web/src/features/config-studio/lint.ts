import type { Diagnostic } from '@codemirror/lint'
import type { Text } from '@codemirror/state'
import type { DiagnosticDetails, Severity } from '@/api/generated'
import { parseSpan, rangeOf } from '@/components/code/spans'

const severities: Record<Severity, Diagnostic['severity']> = {
  ERROR: 'error',
  WARNING: 'warning',
  INFO: 'info',
}

/** The diagnostics located in `file`, as ranges of its document. */
export function editorDiagnostics(
  diagnostics: readonly DiagnosticDetails[],
  file: string,
  doc: Text,
): Diagnostic[] {
  return diagnostics.flatMap((diagnostic) => {
    const span = parseSpan(diagnostic.source_span)
    if (!span || span.file !== file) {
      return []
    }
    const message = diagnostic.help
      ? `${diagnostic.message}\n${diagnostic.help}`
      : diagnostic.message
    return [
      {
        ...rangeOf(doc, span),
        severity: severities[diagnostic.severity] ?? 'error',
        message,
        source: diagnostic.code,
      },
    ]
  })
}
