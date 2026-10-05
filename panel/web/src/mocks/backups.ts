import { http, HttpResponse, type AnyHandler } from 'msw'
import type { BackupDetails, BackupRestore, NewBackup } from '@/api/generated'

/** Backups kept in memory; a new one is taken a few seconds after it is asked for. */
export function backupHandlers(): AnyHandler[] {
  const backups: BackupDetails[] = [
    {
      id: '6f1c7a52-2b8e-4d6b-9a33-0d3c58f1e2a4',
      contents: ['configuration', 'certificates'],
      state: 'completed',
      requested_by: 'admin',
      requested_at: new Date(Date.now() - 86_400_000).toISOString(),
      finished_at: new Date(Date.now() - 86_395_000).toISOString(),
      size_bytes: 482_304,
      sha256: 'ab'.repeat(32),
      files: 4,
      product_version: '0.1.0',
    },
  ]

  return [
    http.get('*/api/v1/backups', () => HttpResponse.json({ backups })),
    http.post('*/api/v1/backups', async ({ request }) => {
      const wanted = (await request.json()) as NewBackup
      const backup: BackupDetails = {
        id: crypto.randomUUID(),
        contents: wanted.contents,
        site_path: wanted.site_path ?? undefined,
        state: 'running',
        requested_by: 'admin',
        requested_at: new Date().toISOString(),
        size_bytes: 0,
        sha256: '',
        files: 0,
        product_version: '0.1.0',
      }
      backups.unshift(backup)
      setTimeout(() => {
        Object.assign(backup, {
          state: 'completed',
          finished_at: new Date().toISOString(),
          size_bytes: 1_048_576,
          sha256: 'cd'.repeat(32),
          files: 9,
        })
      }, 3000)
      return HttpResponse.json(backup, {
        status: 202,
        headers: { location: `/api/v1/backups/${backup.id}` },
      })
    }),
    http.post('*/api/v1/backups/:id/restores', async ({ request }) => {
      const wanted = (await request.json()) as BackupRestore
      return HttpResponse.json(
        wanted.target === 'sites'
          ? { target: 'sites', site_path: wanted.site_path, files: 9, bytes: 1_048_576 }
          : { target: 'configuration', draft_version: 7 },
      )
    }),
    http.delete('*/api/v1/backups/:id', ({ params }) => {
      const index = backups.findIndex((backup) => backup.id === params.id)
      if (index >= 0) {
        backups.splice(index, 1)
      }
      return new HttpResponse(null, { status: 204 })
    }),
  ]
}
