import type { RevisionsMessages } from './zh-CN'

const en: RevisionsMessages = {
  revisions: {
    title: 'Revisions',
    description:
      'Every apply records a revision to compare, restore into the draft or roll back to.',
    emptyTitle: 'No revisions yet',
    emptyDetail: 'Each time the draft is applied, it is recorded here.',
    columns: {
      id: 'Revision',
      outcome: 'Outcome',
      note: 'Note',
      author: 'Author',
      created: 'Created',
      draft: 'Draft',
    },
    outcome: {
      applying: 'Applying',
      active: 'Active',
      superseded: 'Superseded',
      rejected: 'Rejected',
      failed: 'Failed',
    },
    detail: 'Revision #{id}',
    settledAt: 'Settled',
    draftVersion: 'Draft version',
    languageVersion: 'Language version',
    gatewayRevision: 'Gateway revision',
    contentHash: 'Content hash',
    snapshotHash: 'Snapshot hash',
    diagnostics: 'Why it did not run',
    noNote: 'No note',
    editNote: 'Edit note',
    noteSaved: 'Note saved',
    tabs: {
      changes: 'Changes',
      files: 'Files',
    },
    against: 'Compare with',
    againstPrevious: 'Previous revision',
    againstActive: 'Active revision',
    againstDraft: 'Current draft',
    identical: 'They are identical',
    restore: 'Restore to draft',
    restoreTitle: 'Restore revision #{id} into the draft?',
    restoreDetail: "The draft is replaced with this revision's files; apply it to make it run.",
    restored: 'Restored revision #{id} into the draft',
    rollback: 'Roll back',
    rollbackTitle: 'Roll back to revision #{id}?',
    rollbackDetail: 'This revision is restored into the draft and applied as a new revision.',
    rollbackReason: 'Reason',
    rollbackNote: 'Roll back to revision #{id}',
    rolledBack: 'Rolled back; revision #{revision} is active',
  },
}

export default en
