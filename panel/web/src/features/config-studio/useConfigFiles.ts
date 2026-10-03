import { computed, ref, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { check, type ConfigSource, type DiagnosticDetails } from '@/api/generated'
import {
  formatMutation,
  replaceSourceMutation,
  sourceOptions,
} from '@/api/generated/@tanstack/vue-query.gen'

/** The file every configuration starts from. */
export const ENTRY = 'main.conf'

/** A relative `.conf` path whose segments are plain names, so never `.` or `..`. */
export function isFilePath(path: string): boolean {
  return (
    path.endsWith('.conf') &&
    path.split('/').every((segment) => /^[A-Za-z0-9_][A-Za-z0-9._-]*$/.test(segment))
  )
}

/** `main.conf` first, then the other files by path. */
export function sortPaths(paths: Iterable<string>): string[] {
  return [...paths].sort((left, right) =>
    left === ENTRY ? -1 : right === ENTRY ? 1 : left.localeCompare(right),
  )
}

/**
 * The draft's files being edited: a working copy against the files last
 * read or saved, the entity tag that saving must match, and the problems
 * the last check found.
 */
export function useConfigFiles() {
  const source = useQuery(sourceOptions())
  const files = ref<Record<string, string>>({})
  const saved = ref<Record<string, string>>({})
  const etag = ref<string>()
  const version = ref<number>()
  const active = ref(ENTRY)
  /** The draft changed elsewhere while this copy has unsaved edits. */
  const stale = ref(false)
  const problems = ref<DiagnosticDetails[]>([])
  const checking = ref(false)
  let checks = 0

  const paths = computed(() => sortPaths(Object.keys(files.value)))
  const changed = computed(() => {
    const all = new Set([...Object.keys(files.value), ...Object.keys(saved.value)])
    return new Set([...all].filter((path) => files.value[path] !== saved.value[path]))
  })
  const dirty = computed(() => changed.value.size > 0)

  function adopt(data: ConfigSource) {
    files.value = { ...data.files }
    saved.value = { ...data.files }
    etag.value = data.etag
    version.value = data.version
    stale.value = false
    if (!(active.value in data.files)) {
      active.value = ENTRY
    }
  }

  watch(
    () => source.data.value,
    (data) => {
      if (!data) {
        return
      }
      if (etag.value === undefined || !dirty.value) {
        adopt(data)
      } else if (data.etag !== etag.value) {
        stale.value = true
      }
    },
    { immediate: true },
  )

  /** Checks the working copy; results of older checks are discarded. */
  async function lint(): Promise<DiagnosticDetails[]> {
    const run = ++checks
    checking.value = true
    try {
      const { data } = await check({ body: { files: files.value }, throwOnError: true })
      if (run === checks) {
        problems.value = data.diagnostics
      }
    } catch {
      // An unreachable API keeps the last known problems.
    } finally {
      if (run === checks) {
        checking.value = false
      }
    }
    return problems.value
  }

  function write(path: string, text: string) {
    files.value = { ...files.value, [path]: text }
  }

  function add(path: string) {
    write(path, '')
    active.value = path
  }

  function remove(path: string) {
    const { [path]: _, ...rest } = files.value
    files.value = rest
    if (active.value === path) {
      active.value = ENTRY
    }
  }

  function revert() {
    files.value = { ...saved.value }
    if (!(active.value in files.value)) {
      active.value = ENTRY
    }
  }

  async function reload() {
    const result = await source.refetch()
    if (result.data) {
      adopt(result.data)
    }
  }

  return {
    source,
    files,
    saved,
    etag,
    version,
    active,
    stale,
    problems,
    checking,
    paths,
    changed,
    dirty,
    adopt,
    lint,
    write,
    add,
    remove,
    revert,
    reload,
    save: useMutation(replaceSourceMutation()),
    format: useMutation(formatMutation()),
  }
}
