import { http, HttpResponse, type AnyHandler } from 'msw'
import type {
  DirectoriesView,
  GatewayServiceView,
  HostAgentView,
  ListenersView,
} from '@/api/generated'

const MIB = 1024 * 1024

/** A connected host agent whose capabilities are in each state the console shows. */
export function hostAgentHandlers(): AnyHandler[] {
  const agent: HostAgentView = {
    status: 'connected',
    build: '0.1.0',
    hostname: 'edge-1',
    capabilities: [
      { capability: 'directories', state: 'available', detail: '' },
      { capability: 'listeners', state: 'available', detail: '' },
      { capability: 'gateway_service', state: 'available', detail: '' },
      {
        capability: 'containers',
        state: 'denied',
        detail: 'add the agent to the group that owns the engine socket',
      },
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
  const listeners: ListenersView = {
    observed_at: new Date().toISOString(),
    listeners: [
      {
        address: '0.0.0.0',
        port: 80,
        uid: 0,
        processes: [{ pid: 912, name: 'nginx', executable: '/usr/sbin/nginx', uid: 0 }],
      },
      {
        address: '0.0.0.0',
        port: 443,
        uid: 0,
        processes: [{ pid: 1204, name: 'gatewayd', executable: '/usr/local/bin/gatewayd', uid: 0 }],
      },
    ],
  }
  const service: GatewayServiceView = {
    observed_at: new Date().toISOString(),
    supervisor: 'container',
    container: {
      engine: 'docker',
      id: '4f1c2b7d9e0a',
      name: 'pingora-panel-gatewayd-1',
      image: 'localhost/pingora-panel:dev',
      state: 'running',
      status: 'Up 3 days',
      health: null,
      started_at: new Date(Date.now() - 3 * 86_400_000).toISOString(),
      finished_at: null,
      exit_code: null,
      restarts: 0,
    },
  }
  return [
    http.get('*/api/v1/host/agent', () => HttpResponse.json(agent)),
    http.get('*/api/v1/host/gateway-service', () => HttpResponse.json(service)),
    http.post('*/api/v1/host/gateway-service/:action', ({ params }) => {
      const stopped = params.action === 'stop'
      const now = new Date().toISOString()
      Object.assign(service.container, {
        state: stopped ? 'exited' : 'running',
        status: stopped ? 'Exited (0) 1 second ago' : 'Up 1 second',
        started_at: stopped ? service.container.started_at : now,
        finished_at: stopped ? now : service.container.finished_at,
        exit_code: stopped ? 0 : service.container.exit_code,
      })
      service.observed_at = now
      return HttpResponse.json(service)
    }),
    http.get('*/api/v1/host/listeners', () => HttpResponse.json(listeners)),
    http.get('*/api/v1/host/directories', () => HttpResponse.json(directories)),
  ]
}
