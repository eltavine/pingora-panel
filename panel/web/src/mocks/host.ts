import { http, HttpResponse, type AnyHandler } from 'msw'
import type { DirectoriesView, HostAgentView } from '@/api/generated'

const MIB = 1024 * 1024

/** A connected host agent whose capabilities are in each state the console shows. */
export function hostAgentHandlers(): AnyHandler[] {
  const agent: HostAgentView = {
    status: 'connected',
    build: '0.1.0',
    hostname: 'edge-1',
    capabilities: [
      { capability: 'directories', state: 'available', detail: '' },
      {
        capability: 'listeners',
        state: 'denied',
        detail: 'grant CAP_DAC_READ_SEARCH and CAP_SYS_PTRACE',
      },
      { capability: 'gateway_unit', state: 'not_enabled', detail: '' },
      { capability: 'containers', state: 'not_enabled', detail: '' },
    ],
  }
  const directories: DirectoriesView = {
    observed_at: new Date().toISOString(),
    directories: [
      {
        kind: 'configuration',
        path: '/var/lib/pingora-panel/gateway',
        present: true,
        bytes: 3.2 * MIB,
        files: 148,
        unreadable: 0,
        truncated: false,
      },
      {
        kind: 'logs',
        path: '/var/log/pingora-panel',
        present: true,
        bytes: 812 * MIB,
        files: 37,
        unreadable: 2,
        truncated: false,
      },
      {
        kind: 'certificates',
        path: '/var/lib/pingora-panel/gateway-secrets',
        present: true,
        bytes: 96 * 1024,
        files: 24,
        unreadable: 0,
        truncated: false,
      },
    ],
  }
  return [
    http.get('*/api/v1/host/agent', () => HttpResponse.json(agent)),
    http.get('*/api/v1/host/directories', () => HttpResponse.json(directories)),
  ]
}
