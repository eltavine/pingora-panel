# 0035: Backups

Status: accepted. Builds on [ADR 0008](0008-durable-jobs.md),
[ADR 0032](0032-one-control-plane-process-on-sqlite.md) and
[ADR 0034](0034-site-files.md).

## Context

An installation keeps what it cannot lose in the control plane's SQLite
files (ADR 0032), one per module, and in the static sites' directory
(ADR 0034). The specification asks for backups of the configuration, the
certificates, the databases and the sites' directories, for restoring the
configuration and a site's directory, and for exporting and importing the
whole configuration; a restore must work on an empty host and prove the
active revision, the certificates it references, the audit chain and the
protocol revisions. Copying a database file while its module writes it
copies a torn file, and its write-ahead log besides.

## Decision

**Archives.** A backup is one archive, a tar archive compressed with
Zstandard (`.tar.zst`) that standard tools read. Its first member is
`manifest.json`: the archive's layout, the product's version, when it was
taken, what it holds, every directory, and each file's size and SHA-256.
Only files and directories are archived, and an archive is checked whole
against its manifest before anything is restored from it. Databases are
under `databases/`, named after their modules, and the sites' directory
under `sites/`.

**Consistency.** Each database is copied with SQLite's `VACUUM INTO`, a
consistent snapshot taken while its module goes on writing; a module never
writes another's file, so each copy is consistent on its own.

**Kinds.** A backup holds any of: the configuration's database; the
automation module's, which keeps the certificates with their keys sealed;
every database; the sites' directory or one directory below it. A backup
holding the configuration's database also carries the draft and the active
revision as configuration bundles under `configuration/`, which the API reads
through the configuration module when the backup is asked for, so no module
reads another's tables. The master keys, the password pepper and the
bootstrap token are never in an archive: sealed values stay sealed, and
restoring them needs the installation's master keys.

**Where.** Archives are kept in the control plane's data directory, written
by a durable job of the automation module and kept to a number of finished
backups, older ones removed as new ones finish. They are listed, downloaded
with their digest (RFC 9530) and deleted through the API; taking them
regularly is left to the host's timers calling the command line.

**Restoring.** A site's directory is restored while the panel runs: the
archive's copy is unpacked beside it and renamed over it, so the gateway
serves the old files or the new ones and never a mix. The configuration is
restored as a change of the draft: the archive's active revision becomes the
draft, which is validated and applied as any other, approvals included.
Databases are restored with the control plane stopped: `panel-control
restore` checks each member against the manifest and SQLite's integrity
check, refuses databases of modules the release does not run or at a schema
newer than it reaches, keeps the files it replaces, with their write-ahead
logs, beside them and installs the copies; the next start migrates them
forward and verifies the audit chain.

**The configuration as a bundle.** Exporting the configuration writes the
draft's files in the configuration language with its version, which another
installation imports as a change of its draft. Certificates are not in it;
TLS profiles name them.

**Who.** `backups.read` lists backups; `backups.manage` takes, downloads,
restores and deletes them, since an archive holds everything the databases
do. The audit trail records each, refused or not.

## Alternatives

- Copying the database files: torn copies and a separate write-ahead log.
- `pg_dump`-style logical dumps: the control plane is on SQLite, whose own
  snapshot is consistent and restores by copying a file.
- Keeping the master keys in the archive: one stolen archive would unseal
  every key in it.

## Consequences

- Restoring databases means a short stop of the control plane; the gateway
  keeps serving its last configuration meanwhile.
- An archive is as sensitive as the data directory: it holds password hashes
  and sealed keys, and is downloaded only by those who may take backups.
