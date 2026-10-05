/** Saves `blob` as a file named `name`. */
export function downloadBlob(name: string, blob: Blob) {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = name
  anchor.click()
  setTimeout(() => URL.revokeObjectURL(url), 0)
}

/** Saves `content` as a file named `name`. */
export function downloadFile(name: string, content: string, type: string) {
  downloadBlob(name, new Blob([content], { type }))
}

/** Saves `value` as a pretty-printed JSON file named `name`. */
export function downloadJson(name: string, value: unknown) {
  downloadFile(name, `${JSON.stringify(value, null, 2)}\n`, 'application/json')
}
