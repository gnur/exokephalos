import Dexie, { type EntityTable } from 'dexie';
import type { Action, Item, OutboxEntry, View } from './types';

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
  }
}

export const db = new ExoDB();

export async function getRevision(): Promise<number> {
  const value = (await db.meta.get('revision'))?.value;
  return typeof value === 'number' ? value : 0;
}

export async function setRevision(revision: number) {
  await db.meta.put({ key: 'revision', value: revision });
}
