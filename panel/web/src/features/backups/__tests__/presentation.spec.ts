import { describe, expect, it } from 'vitest'
import type { BackupDetails } from '@/api/generated'
import { archiveUrl, restoresConfiguration, restoresSites, sitePath, taking } from '../presentation'

function backup(overrides: Partial<BackupDetails>): BackupDetails {
  return {
    id: 'b-1',
    contents: ['configuration'],
    state: 'completed',
    requested_by: 'ops',
    requested_at: '2027-01-15T08:00:00Z',
    size_bytes: 1024,
    sha256: 'ab'.repeat(32),
    files: 3,
    product_version: '1.2.3',
    ...overrides,
  }
}

describe('backups', () => {
  it('are refreshed while one is being taken', () => {
    expect(taking([backup({}), backup({ state: 'running' })])).toBe(true)
    expect(taking([backup({}), backup({ state: 'failed' })])).toBe(false)
  })

  it('restore only what they hold once taken', () => {
    expect(restoresConfiguration(backup({ contents: ['databases'] }))).toBe(true)
    expect(restoresConfiguration(backup({ contents: ['sites'] }))).toBe(false)
    expect(restoresConfiguration(backup({ state: 'pending' }))).toBe(false)
    expect(restoresSites(backup({ contents: ['sites'] }))).toBe(true)
    expect(restoresSites(backup({ contents: ['certificates'] }))).toBe(false)
  })

  it('name their archive and the directories they restore', () => {
    expect(archiveUrl(backup({ id: 'a b' }))).toBe('/api/v1/backups/a%20b/archive')
    expect(sitePath(' /shop/assets/ ')).toBe('shop/assets')
  })
})
