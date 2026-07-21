import Dexie, { type EntityTable } from 'dexie';
import type { Action, Item, OutboxEntry, SyncDevice, View } from './types';

export type Meta = {
  key: string;
  value: unknown;
};

export class ExoDB extends Dexie {
  items!: EntityTable<Item, 'id'>;
  outbox!: EntityTable<OutboxEntry, 'id'>;
  views!: EntityTable<View, 'id'>;
  actions!: EntityTable<Action, 'name'>;
  meta!: EntityTable<Meta, 'key'>;

  constructor() {
    super('exokephalos');
    this.version(1).stores({
      items: 'id, type, title, path, updated_at, deleted',
      outbox: 'id, item_id, status, created_at, updated_at',
      views: 'id',
      actions: 'name',
      meta: 'key',
    });
		this.version(2).stores({
			items: 'id, type, title, path, updated_at, deleted',
			outbox: 'id, item_id, status, created_at, updated_at',
			views: 'id', actions: 'name', meta: 'key',
		});
  }
}

function inferredDeviceLabel() {
  const ua = navigator.userAgent;
  const os = /Mac OS X/.test(ua) ? 'macOS' : /Windows/.test(ua) ? 'Windows' : /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iOS' : 'browser';
  return `${os} browser`;
}

export async function ensureSyncDevice(): Promise<SyncDevice> {
  const existing = (await db.meta.get('sync_device'))?.value;
  if (typeof existing === 'object' && existing && 'id' in existing) return existing as SyncDevice;
  const device: SyncDevice = { id: crypto.randomUUID(), label: inferredDeviceLabel(), logical: 0, physical_ms: 0 };
  await db.meta.put({ key: 'sync_device', value: device });
  return device;
}

export async function renameSyncDevice(label: string) {
  const device = await ensureSyncDevice();
  device.label = label.trim() || device.label;
  await db.meta.put({ key: 'sync_device', value: device });
  return device;
}

export const db = new ExoDB();

export async function getRevision(): Promise<number> {
  const value = (await db.meta.get('revision'))?.value;
  return typeof value === 'number' ? value : 0;
}

export async function setRevision(revision: number) {
  await db.meta.put({ key: 'revision', value: revision });
}
