import { http, HttpResponse, type AnyHandler } from 'msw'
import type {
  DirectoriesView,
  GatewayUnitView,
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
      { capability: 'gateway_unit', state: 'available', detail: '' },
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
  const unit: GatewayUnitView = {
    name: 'pingora-panel-gatewayd.service',
    description: 'Pingora Panel gateway',
    load_state: 'loaded',
    active_state: 'active',
    sub_state: 'running',
    unit_file_state: 'enabled',
    main_pid: 1204,
    active_since: new Date(Date.now() - 3 * 86_400_000).toISOString(),
    restarts: 0,
    result: 'success',
  }
  return [
    http.get('*/api/v1/host/agent', () => HttpResponse.json(agent)),
    http.get('*/api/v1/host/gateway-unit', () => HttpResponse.json(unit)),
    http.post('*/api/v1/host/gateway-unit/:action', ({ params }) => {
      const stopped = params.action === 'stop'
      Object.assign(unit, {
        active_state: stopped ? 'inactive' : 'active',
        sub_state: stopped ? 'dead' : 'running',
        main_pid: stopped ? null : (unit.main_pid ?? 1204),
        active_since: stopped ? unit.active_since : new Date().toISOString(),
      })
      return HttpResponse.json(unit)
    }),
    http.get('*/api/v1/host/listeners', () => HttpResponse.json(listeners)),
    http.get('*/api/v1/host/directories', () => HttpResponse.json(directories)),
  ]
}
