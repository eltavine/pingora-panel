import type { LogRecordItem } from '@/api/generated'

/** The message of an error record: from its JSON line, else from its fields. */
export function messageOf(record: LogRecordItem): string {
  try {
    const parsed: unknown = JSON.parse(record.line)
    if (typeof parsed === 'object' && parsed !== null && 'message' in parsed) {
      return String(parsed.message)
    }
  } catch {
    // Combined lines are not JSON.
  }
  return record.fields.message ?? record.line
}

/** What a record says in a line: an access record's request, an error record's message. */
export function summaryOf(record: LogRecordItem): string {
  return record.kind === 'access'
    ? [record.method, record.path].filter(Boolean).join(' ') || record.line
    : messageOf(record)
}
