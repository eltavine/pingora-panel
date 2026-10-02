import { computed, ref, shallowRef, watch } from 'vue'
import { useMutation, useQueryClient } from '@tanstack/vue-query'
import {
  abortMutation,
  activateMutation,
  prepareMutation,
  statusQueryKey,
  validateMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import type {
  ActivatedResponse,
  PreparedResponse,
  SnapshotEnvelope,
  ValidationResponse,
} from '@/api/generated'
import { commandHeaders, newIdempotencyKey } from '@/lib/command'

export type ParsedDocument = { ok: true; value: unknown } | { ok: false; reason: string }

export function parseDocument(text: string): ParsedDocument {
  if (text.trim() === '') {
    return { ok: false, reason: '' }
  }
  try {
    return { ok: true, value: JSON.parse(text) }
  } catch (error) {
    return { ok: false, reason: error instanceof Error ? error.message : String(error) }
  }
}

/**
 * Validate → prepare → activate or abort, one snapshot at a time.
 *
 * Each command keeps its idempotency key until it succeeds or its input
 * changes, so retrying after a lost response cannot apply it twice.
 */
export function usePublishWorkflow() {
  const queryClient = useQueryClient()
  const schemaVersion = ref('v1')
  const documentText = ref('')
  const expectedHash = ref('')
  const parsed = computed(() => parseDocument(documentText.value))
  const validation = shallowRef<ValidationResponse | null>(null)
  const prepared = shallowRef<PreparedResponse | null>(null)
  const activated = shallowRef<ActivatedResponse | null>(null)
  const aborted = ref(false)
  let prepareKey = newIdempotencyKey()
  let activateKey = newIdempotencyKey()
  let abortKey = newIdempotencyKey()

  const validate = useMutation(validateMutation())
  const prepare = useMutation(prepareMutation())
  const activate = useMutation(activateMutation())
  const abort = useMutation(abortMutation())

  watch([documentText, schemaVersion], () => {
    validation.value = null
    prepared.value = null
    prepareKey = newIdempotencyKey()
    validate.reset()
    prepare.reset()
  })

  function envelope(): SnapshotEnvelope | null {
    return parsed.value.ok
      ? { schema_version: schemaVersion.value, snapshot: parsed.value.value }
      : null
  }

  function runValidate() {
    const body = envelope()
    if (body) {
      validate.mutate({ body }, { onSuccess: (result) => (validation.value = result) })
    }
  }

  function runPrepare() {
    const body = envelope()
    if (!body) {
      return
    }
    activated.value = null
    aborted.value = false
    prepare.mutate(
      { body, headers: commandHeaders(prepareKey) },
      {
        onSuccess: (result) => {
          prepared.value = result
          prepareKey = newIdempotencyKey()
          activateKey = newIdempotencyKey()
          abortKey = newIdempotencyKey()
          activate.reset()
          abort.reset()
        },
      },
    )
  }

  function runActivate() {
    const token = prepared.value?.prepare_token
    if (!token) {
      return
    }
    activate.mutate(
      {
        body: { prepare_token: token, expected_active_hash: expectedHash.value.trim() || null },
        headers: commandHeaders(activateKey),
      },
      {
        onSuccess: (result) => {
          activated.value = result
          prepared.value = null
          expectedHash.value = result.content_hash
          void queryClient.invalidateQueries({ queryKey: statusQueryKey() })
        },
      },
    )
  }

  function runAbort() {
    const token = prepared.value?.prepare_token
    if (!token) {
      return
    }
    abort.mutate(
      { body: { prepare_token: token }, headers: commandHeaders(abortKey) },
      {
        onSuccess: () => {
          prepared.value = null
          aborted.value = true
          void queryClient.invalidateQueries({ queryKey: statusQueryKey() })
        },
      },
    )
  }

  return {
    schemaVersion,
    documentText,
    expectedHash,
    parsed,
    validation,
    prepared,
    activated,
    aborted,
    validate,
    prepare,
    activate,
    abort,
    runValidate,
    runPrepare,
    runActivate,
    runAbort,
  }
}
