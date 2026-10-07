import type { BackupsMessages } from './zh-CN'

const en: BackupsMessages = {
  backups: {
    title: 'Backups',
    description:
      'Archives of the databases and the sites’ directory, checked against their manifests and restored in place. Master keys are never in them.',
    refresh: 'Refresh',
    take: 'Take a backup',
    takeTitle: 'Take a backup',
    takeDetail:
      'It is taken in the background and kept in the control plane’s data directory; older ones are removed as new ones are taken.',
    takingNow: 'Taking the backup',
    takeFailed: 'The backup was not started',
    taken: 'Taken',
    by: 'by {actor}',
    holds: 'Holds',
    state: 'State',
    size: 'Size',
    actionsFor: 'Actions for the backup of {time}',
    download: 'Download the archive',
    restore: 'Restore',
    restoreSites: 'Restore a site directory…',
    restoreSitesTitle: 'Restore a site directory',
    restoreSitesDetail:
      'Replaces the directory below the sites’ directory with the backup’s copy; the gateway serves the old files or the new ones, never a mix.',
    sitesRestored: 'Restored {path}: {count} file | Restored {path}: {count} files',
    restoreConfiguration: 'Restore the configuration…',
    restoreConfigurationTitle: 'Restore the configuration',
    restoreConfigurationDetail:
      'Saves the configuration the gateway ran when this backup was taken as the draft, replacing it. Review and apply it as any other change.',
    configurationRestored: 'Saved as draft v{version}',
    review: 'Review',
    restoreFailed: 'Nothing was restored',
    remove: 'Remove',
    removeTitle: 'Remove this backup?',
    removeDetail: 'Its archive is deleted; this cannot be undone.',
    removed: 'Removed the backup',
    removeFailed: 'The backup was not removed',
    empty: 'No backups yet',
    emptyDetail: 'Take one to keep the configuration, certificates, databases or sites.',
    siteDirectory: 'Directory below the sites’ directory',
    allSites: 'Every site when empty',
    contents: {
      configuration: 'Configuration',
      certificates: 'Certificates',
      databases: 'Databases',
      sites: 'Sites',
    },
    explained: {
      configuration: 'Its database, with the draft and the active revision as bundles.',
      certificates: 'Certificates with their keys sealed, ACME accounts and DNS providers.',
      databases: 'Every module’s database, the audit trail and accounts included.',
      sites: 'The files the gateway serves, or one directory of them.',
    },
    states: {
      pending: 'Waiting',
      running: 'Being taken',
      completed: 'Taken',
      failed: 'Failed',
    },
  },
}

export default en
