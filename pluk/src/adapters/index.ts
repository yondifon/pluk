import type { Adapter } from "./types.js";
import { sqlAdapters } from "./sql/index.js";
import { linearAdapter } from "./linear/index.js";
import { sentryAdapter } from "./sentry/index.js";
import { sshAdapter } from "./ssh/index.js";

// The adapter registry. To add a service: build an Adapter module and register
// it here. Nothing else (store, MCP transport, REST layer, UI) needs editing.
const registry = new Map<string, Adapter>();

function register(adapters: Adapter[]): void {
  for (const adapter of adapters) {
    if (registry.has(adapter.id)) throw new Error(`Duplicate adapter id: ${adapter.id}`);
    registry.set(adapter.id, adapter);
  }
}

register(sqlAdapters);
register([linearAdapter, sentryAdapter, sshAdapter]);

export function getAdapter(type: string): Adapter | undefined {
  return registry.get(type);
}

export function listAdapters(): Adapter[] {
  return [...registry.values()];
}

export type { Adapter, ConfigField, FieldType, PolicyKind } from "./types.js";
