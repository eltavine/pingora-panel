import { http, HttpResponse, type AnyHandler } from 'msw'
import type { SiteDirectoryView, SiteEntryView, SiteFileWrittenView } from '@/api/generated'

/** A shop and a blog, kept in memory by path. */
export function siteFileHandlers(): AnyHandler[] {
  const now = () => new Date().toISOString()
  const files = new Map<string, { content: Uint8Array; modified: string }>()
  const directories = new Set(['shop', 'shop/assets', 'blog'])
  const encoder = new TextEncoder()
  const put = (path: string, text: string) =>
    files.set(path, { content: encoder.encode(text), modified: now() })
  put('shop/index.html', '<!doctype html>\n<h1>Shop</h1>\n')
  put('shop/assets/app.css', 'body { margin: 0; }\n')
  put('blog/index.html', '<!doctype html>\n<h1>Blog</h1>\n')

  const normalized = (value: string | null) => (value ?? '').replace(/^\/+|\/+$/g, '')
  const parent = (path: string) => path.slice(0, Math.max(path.lastIndexOf('/'), 0))
  const tag = (content: Uint8Array) =>
    `"${content.length}-${content.reduce((sum, byte) => (sum * 31 + byte) >>> 0, 7)}"`
  const problem = (status: number, code: string, detail: string) =>
    HttpResponse.json(
      { type: 'about:blank', title: code, status, code, detail },
      { status, headers: { 'content-type': 'application/problem+json' } },
    )

  return [
    http.get('*/api/v1/site-files', ({ request }) => {
      const path = normalized(new URL(request.url).searchParams.get('path'))
      if (path && !directories.has(path)) {
        return problem(404, 'NOT_FOUND', `/${path} does not exist`)
      }
      const entries: SiteEntryView[] = [
        ...[...directories]
          .filter((directory) => parent(directory) === path && directory !== path)
          .map((directory) => ({
            name: directory.slice(path ? path.length + 1 : 0),
            kind: 'directory' as const,
            size_bytes: 0,
            modified: null,
          })),
        ...[...files]
          .filter(([file]) => parent(file) === path)
          .map(([file, { content, modified }]) => ({
            name: file.slice(path ? path.length + 1 : 0),
            kind: 'file' as const,
            size_bytes: content.length,
            modified,
          })),
      ].sort((left, right) =>
        left.kind === right.kind
          ? left.name.localeCompare(right.name)
          : left.kind === 'directory'
            ? -1
            : 1,
      )
      return HttpResponse.json({ path, entries } satisfies SiteDirectoryView)
    }),
    http.get('*/api/v1/site-files/content', ({ request }) => {
      const path = normalized(new URL(request.url).searchParams.get('path'))
      const file = files.get(path)
      if (!file) {
        return problem(404, 'NOT_FOUND', `/${path} does not exist`)
      }
      return new HttpResponse(file.content, {
        headers: { 'content-type': 'application/octet-stream', etag: tag(file.content) },
      })
    }),
    http.put('*/api/v1/site-files/content', async ({ request }) => {
      const path = normalized(new URL(request.url).searchParams.get('path'))
      const before = files.get(path)
      const ifMatch = request.headers.get('if-match')
      if (request.headers.get('if-none-match') === '*' && before) {
        return problem(412, 'PRECONDITION_FAILED', `/${path} already exists`)
      }
      if (ifMatch && ifMatch !== '*' && (!before || tag(before.content) !== ifMatch)) {
        return problem(412, 'PRECONDITION_FAILED', `/${path} changed since it was read`)
      }
      const content = new Uint8Array(await request.arrayBuffer())
      files.set(path, { content, modified: now() })
      for (let directory = parent(path); directory; directory = parent(directory)) {
        directories.add(directory)
      }
      return HttpResponse.json(
        {
          path,
          size_bytes: content.length,
          sha256: '0'.repeat(64),
          created: !before,
        } satisfies SiteFileWrittenView,
        { status: before ? 200 : 201, headers: { etag: tag(content) } },
      )
    }),
    http.post('*/api/v1/site-files/directories', ({ request }) => {
      let directory = normalized(new URL(request.url).searchParams.get('path'))
      for (; directory; directory = parent(directory)) {
        directories.add(directory)
      }
      return new HttpResponse(null, { status: 204 })
    }),
    http.delete('*/api/v1/site-files', ({ request }) => {
      const url = new URL(request.url)
      const path = normalized(url.searchParams.get('path'))
      const recursive = url.searchParams.get('recursive') === 'true'
      if (files.delete(path)) {
        return HttpResponse.json({ path, kind: 'file', removed: 1 })
      }
      if (!directories.has(path)) {
        return problem(404, 'NOT_FOUND', `/${path} does not exist`)
      }
      const inside = (entry: string) => entry.startsWith(`${path}/`)
      const held = [...files.keys()].filter(inside).length + [...directories].filter(inside).length
      if (held && !recursive) {
        return problem(409, 'CONFLICT', `/${path} is not empty`)
      }
      for (const file of [...files.keys()].filter(inside)) {
        files.delete(file)
      }
      for (const directory of [...directories].filter(inside)) {
        directories.delete(directory)
      }
      directories.delete(path)
      return HttpResponse.json({ path, kind: 'directory', removed: held + 1 })
    }),
  ]
}
