# 0034: Site files

Status: accepted. Builds on [ADR 0012](0012-configuration-language-and-revisions.md)
and [ADR 0014](0014-identity-and-access.md).

## Context

A static site serves the files below its root, a directory below the
gateway's static root. Operators need to see, edit, upload and download those
files without a shell on the host, and nothing else: the specification limits
the file manager to the configuration and to the sites' directories. The
configuration's files are the configuration language's (ADR 0012), which the
console already reads and edits through the draft. A file manager that takes
paths from requests is a path traversal waiting to happen: `..`, an absolute
path or a symbolic link in the tree can each lead out of the directory.

## Decision

**Where.** The control plane mounts the directory the gateway reads static
sites from, read-write, and manages the files below it and nothing else.
Paths are relative to it, written with `/`, each part a name: never empty,
`.`, `..`, absolute or containing a NUL. Every operation opens the root as a
capability (`cap-std`) and resolves paths beneath it, so neither a path nor a
symbolic link in the tree leads out of it, whatever the platform.

**What.** A directory lists its entries with their kind, size and time of
change. A file is read whole, up to 64 MiB, with an entity tag over its
content; text up to 1 MiB is shown for editing. Writing a file replaces it
atomically, through a temporary file renamed over it, and a write that names
an entity tag replaces only the file it read. Directories are created and
files and directories removed, a directory with its contents only when asked.

**Who.** `files.read` lists and reads, `files.write` writes, creates and
removes; operators hold both and viewers the first. The audit trail records
each change, refused or not, with the file's path and, for a write, its size
and digest, never its content.

**The configuration's files** stay in the configuration language's editor,
where a change is a change of the draft, validated and applied as any other.

## Alternatives

- Checking paths as text before opening them: a symbolic link inside the tree
  still leads out, and the check and the open race.
- Letting the agent manage the files: the sites' directory is a volume of the
  installation, not a host path the agent should be granted.
- A general file manager over the host: the specification limits it, and the
  agent refuses paths from requests (ADR 0030).

## Consequences

- The control plane writes what the gateway serves, so a write is served at
  once; there is no draft for static files.
- Files the gateway cannot read, such as ones whose owner or mode the control
  plane cannot set, are a matter for the host; the control plane writes files
  readable by the gateway's group.
