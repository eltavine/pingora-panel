// The licenses of the console's production dependencies, with their texts,
// as the JavaScript part of the third-party notices (ADR 0046). Run from
// `panel/web` after `pnpm install`; it fails on a license not accepted.
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'

/** The licenses deny.toml allows for crates, and those of the console's fonts and helpers. */
const ACCEPTED = new Set([
  'Apache-2.0',
  'BSD-2-Clause',
  'BSD-3-Clause',
  'BSL-1.0',
  'CC0-1.0',
  'ISC',
  'MIT',
  'MIT-0',
  'MPL-2.0',
  'Unicode-3.0',
  'Zlib',
  '0BSD',
  'OFL-1.1',
])

interface Node {
  version: string
  path: string
  dependencies?: Record<string, Node>
}

interface Package {
  name: string
  version: string
  license: string
  text: string
}

/** Whether an SPDX expression of ORs of ANDs can be satisfied by accepted licenses. */
function accepted(expression: string): boolean {
  return expression
    .replace(/[()]/g, '')
    .split(/\s+OR\s+/)
    .some((alternative) => alternative.split(/\s+AND\s+/).every((id) => ACCEPTED.has(id.trim())))
}

function licenseOf(manifest: { license?: unknown; licenses?: unknown }): string {
  const license = manifest.license ?? manifest.licenses
  if (typeof license === 'string') {
    return license
  }
  if (Array.isArray(license)) {
    return license
      .map((item) => (typeof item === 'string' ? item : String(item?.type)))
      .join(' OR ')
  }
  if (license && typeof license === 'object' && 'type' in license) {
    return String(license.type)
  }
  return 'UNKNOWN'
}

function textOf(directory: string): string {
  const file = readdirSync(directory)
    .sort()
    .find((name) => /^(licen[sc]e|copying)([.-]|$)/i.test(name))
  return file ? readFileSync(join(directory, file), 'utf8').trim() : ''
}

function collect(dependencies: Record<string, Node> | undefined, into: Map<string, Package>) {
  for (const [name, node] of Object.entries(dependencies ?? {})) {
    const key = `${name}@${node.version}`
    // Optional packages built for other platforms are listed but not installed.
    if (into.has(key) || !existsSync(join(node.path, 'package.json'))) {
      continue
    }
    const manifest = JSON.parse(readFileSync(join(node.path, 'package.json'), 'utf8'))
    into.set(key, {
      name,
      version: node.version,
      license: licenseOf(manifest),
      text: textOf(node.path),
    })
    collect(node.dependencies, into)
  }
}

function render(packages: Package[]): string {
  const groups = new Map<string, Package[]>()
  for (const item of packages) {
    const key = `${item.license}\n${item.text}`
    groups.set(key, [...(groups.get(key) ?? []), item])
  }
  const sections = [...groups.values()]
    .sort((left, right) =>
      `${left[0]!.license}${left[0]!.name}`.localeCompare(`${right[0]!.license}${right[0]!.name}`),
    )
    .map((members) => {
      const { license, text } = members[0]!
      const users = members.map((item) => `${item.name} ${item.version}`).join(', ')
      const body = text
        ? `\`\`\`text\n${text}\n\`\`\``
        : `These packages declare ${license} and carry no license text of their own.`
      return `### ${license}\n\nUsed by ${users}.\n\n${body}\n`
    })
  return `## JavaScript packages\n\n${sections.join('\n')}`
}

const listed = execFileSync('pnpm', ['list', '--prod', '--json', '--depth', 'Infinity'], {
  encoding: 'utf8',
  maxBuffer: 64 * 1024 * 1024,
})
const packages = new Map<string, Package>()
collect(
  (JSON.parse(listed) as { dependencies?: Record<string, Node> }[])[0]?.dependencies,
  packages,
)
const refused = [...packages.values()].filter((item) => !accepted(item.license))
if (refused.length > 0) {
  for (const item of refused) {
    console.error(`${item.name} ${item.version} is licensed ${item.license}, which is not accepted`)
  }
  process.exit(1)
}
process.stdout.write(render([...packages.values()]))
