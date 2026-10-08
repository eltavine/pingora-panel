import type { SystemMessages } from './zh-CN'

const en: SystemMessages = {
  system: {
    title: 'System',
    description:
      'The versions of what runs, whether an upgrade can start, and a diagnostic bundle with its secrets removed.',
    versions: {
      title: 'Versions',
      description: 'The release, each module, the gateway, the host agent and the deployed images.',
      release: 'Release',
      api: 'API',
      language: 'Configuration language',
      irSchema: 'IR schema',
      gateway: 'Gateway',
      gatewayVersions: '{gateway} · engine {engine} · adapter {adapter}',
      agent: 'Host agent',
      agentVersions: '{build} · protocol {protocol} · {hostname}',
      unavailable: 'Not connected',
      deployment: 'Deployment',
      deployed: '{action} on {engine}, {time}',
      previous: 'from {release}',
      notDeployed: 'Not deployed by the lifecycle tool',
      modules: 'Modules',
      module: 'Module',
      build: 'Build',
      schema: 'Database schema',
      protocols: 'Protocol revisions',
      protocol: '{name} {min}–{max}',
      noModules: 'The service directory lists no modules yet',
      images: 'Deployed images',
      service: 'Service',
      image: 'Image',
      digest: 'Digest',
      localBuild: 'Local build',
      problems: 'Some parts could not be read',
    },
    actions: {
      install: 'Installed',
      upgrade: 'Upgraded',
      rollback: 'Rolled back',
      restore: 'Restored',
    },
    readiness: {
      title: 'Upgrade readiness',
      description:
        'Whether every module is healthy, whether anything is prepared or being applied on the gateway, the newest backup and the space for another.',
      ready: 'An upgrade can start',
      notReady: 'An upgrade cannot start yet',
      hint: 'pingora-panel upgrade on the host runs the same checks first, then backs everything up.',
      checks: {
        modules: 'Modules',
        gateway: 'Gateway',
        backup: 'Backup',
        space: 'Disk space',
      },
      states: {
        pass: 'Passed',
        warn: 'Attention',
        fail: 'Failed',
      },
    },
    diagnostics: {
      title: 'Diagnostic bundle',
      description:
        "One JSON file: versions, readiness, health, the gateway and the host, the configuration's counts, recent failures and audit events. Secrets are removed before it leaves the panel.",
      download: 'Download the bundle',
      downloaded: 'Downloaded {name}',
      failed: 'Could not put the bundle together',
      needs: 'Downloading the bundle needs the platform.diagnose permission.',
      withheld: 'Parts you may not read are left out and named in the file.',
    },
  },
}

export default en
