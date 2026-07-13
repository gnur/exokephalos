export type Frontmatter = Record<string, unknown>;

export type Item = {
  id: string;
  path: string;
  type: string;
  title: string;
  subtitle: string;
  tags: string[];
  frontmatter: Frontmatter;
  body: string;
  raw: string;
  updated_at?: string;
  deleted?: boolean;
};

export type ViewConfig = {
  name?: string;
  key?: string;
  filter?: string;
  show_tags?: boolean;
  title_field?: string;
  subtitle_field?: string;
  sort_field?: string;
  sort_order?: string;
  template?: string;
  preview_template?: string;
  stats_template?: string;
  subviews?: Array<{ name: string; filter: string }>;
};

export type View = {
  id: string;
  config: ViewConfig;
  item_ids: string[];
  subviews?: Array<{ name: string; item_ids: string[] }>;
};

export type Action = {
  name: string;
  description: string;
  filter: string;
};

export type Bootstrap = {
  default_view: string;
  views: View[];
  actions: Action[];
  items: Item[];
  revision: number;
  sync_server_enabled: boolean;
};

export type OutboxStatus = 'pending' | 'syncing' | 'synced' | 'failed';

export type OutboxEntry = {
  id: string;
  op: 'upsert_item' | 'delete_item';
  item_id: string;
  path: string;
  frontmatter?: Frontmatter;
  body?: string;
  status: OutboxStatus;
  attempts: number;
  error?: string;
  created_at: string;
  updated_at: string;
};

export type SyncClient = {
  id: string;
  label: string;
  status: string;
  created_at: string;
  approved_at: string;
};
