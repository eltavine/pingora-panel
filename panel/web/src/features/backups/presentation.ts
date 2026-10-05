import type { BackupContentName, BackupDetails } from '@/api/generated'

/** What a backup can hold, in the order offered. */
export const CONTENTS: readonly BackupContentName[] = [
  'configuration',
  'certificates',
  'databases',
  'sites',
]

/** Whether any backup is still being taken, so the list is refreshed. */
export function taking(backups: readonly BackupDetails[]): boolean {
  return backups.some((backup) => backup.state === 'pending' || backup.state === 'running')
}

/** Whether the configuration can be restored from a backup. */
export function restoresConfiguration(backup: BackupDetails): boolean {
  return (
    backup.state === 'completed' &&
    backup.contents.some((content) => content === 'configuration' || content === 'databases')
  )
}

/** Whether a directory of the sites can be restored from a backup. */
export function restoresSites(backup: BackupDetails): boolean {
  return backup.state === 'completed' && backup.contents.includes('sites')
}

/** Where a backup's archive is downloaded from. */
export function archiveUrl(backup: BackupDetails): string {
  return `/api/v1/backups/${encodeURIComponent(backup.id)}/archive`
}

/** A directory below the sites' directory, without leading or trailing slashes. */
export function sitePath(value: string): string {
  return value.trim().replace(/^\/+|\/+$/g, '')
}
