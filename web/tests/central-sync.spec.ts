import { expect, test, type BrowserContext, type Page } from '@playwright/test';

async function connect(page: Page, clientId: string) {
  await page.goto('/');
  const connect = page.getByRole('button', { name: 'Connect to workspace' });
  await expect(connect).toBeVisible();
  const input = page.getByLabel('Client ID');
  if (await input.isVisible()) await input.fill(clientId);
  await connect.click();
  await expect(page.getByRole('button', { name: 'New note' })).toBeVisible();
}

async function createNote(page: Page, title: string, body: string) {
  await page.getByRole('button', { name: 'New note' }).click();
  await page.getByRole('textbox', { name: 'Title', exact: true }).fill(title);
  await page.getByLabel('Frontmatter and Markdown').fill(`---\ntitle: ${title}\ntype: note\ntags: []\n---\n${body}`);
  await page.getByRole('button', { name: 'Save note' }).click();
  await expect(page.getByText(title, { exact: true })).toBeVisible();
}

async function wipe(context: BrowserContext) {
  await context.setOffline(false);
  await context.clearCookies();
}

test('restores the durable Automerge replica before reconnecting', async ({ page, context }) => {
  const title = `Durable ${Date.now()}`;
  await connect(page, 'durable-browser');
  await createNote(page, title, 'available after reload');
  await context.setOffline(true);
  await page.reload();
  await expect(page.getByText(title, { exact: true })).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText('Working from the durable local replica')).toBeVisible();
  await wipe(context);
});

test('synchronizes two browser replicas through same-origin api sync', async ({ browser }) => {
  const firstContext = await browser.newContext();
  const secondContext = await browser.newContext();
  const first = await firstContext.newPage();
  const second = await secondContext.newPage();
  const title = `Converged ${Date.now()}`;
  try {
    await connect(first, 'browser-one');
    await connect(second, 'browser-two');
    await createNote(first, title, 'through the central server');
    await expect(second.getByText(title, { exact: true })).toBeVisible({ timeout: 45_000 });
  } finally {
    await firstContext.close();
    await secondContext.close();
  }
});

test('converges with a native client through the shared api sync endpoint', async ({ page }) => {
  test.skip(process.env.XO_NATIVE_FIXTURE !== '1', 'requires the native fixture imported by CI');
  await connect(page, 'native-reader');
  await expect(page.getByText('Native browser fixture', { exact: true })).toBeVisible({ timeout: 45_000 });
  await page.getByText('Native browser fixture', { exact: true }).click();
  await expect(page.locator('.markdown-preview')).toContainText('synchronized from the native client');
});

test('receives authoritative HTTP API changes through api sync', async ({ page, request }) => {
  const title = `API ${Date.now()}`;
  await connect(page, 'api-browser');
  await createNote(page, title, 'created in browser');
  await page.getByText(title, { exact: true }).click();
  await expect(page).toHaveURL(/\/views\/notes\/[a-z2-7]{7}/);
  const noteId = new URL(page.url()).pathname.split('/').at(-1)!;
  const fetched = await request.get(`/api/items/${noteId}`);
  expect(fetched.ok()).toBeTruthy();
  expect((await fetched.json()).body).toContain('created in browser');

  const patched = await request.patch(`/api/items/${noteId}`, {
    headers: { 'content-type': 'application/json' },
    data: { body: 'updated by the HTTP API' },
  });
  expect(patched.ok()).toBeTruthy();
  await expect(page.locator('.markdown-preview')).toContainText('updated by the HTTP API', { timeout: 45_000 });
});

test('queues an offline mutation and synchronizes it after reconnect', async ({ browser }) => {
  const writerContext = await browser.newContext();
  const readerContext = await browser.newContext();
  const writer = await writerContext.newPage();
  const reader = await readerContext.newPage();
  const title = `Offline ${Date.now()}`;
  try {
    await connect(writer, 'offline-writer');
    await connect(reader, 'online-reader');
    await writerContext.setOffline(true);
    await createNote(writer, title, 'created without connectivity');
    await expect(reader.getByText(title, { exact: true })).not.toBeVisible();
    await writerContext.setOffline(false);
    await expect(reader.getByText(title, { exact: true })).toBeVisible({ timeout: 45_000 });
  } finally {
    await writerContext.setOffline(false);
    await writerContext.close();
    await readerContext.close();
  }
});

test('retains concurrent browser revisions after offline edits', async ({ browser }) => {
  const firstContext = await browser.newContext();
  const secondContext = await browser.newContext();
  const first = await firstContext.newPage();
  const second = await secondContext.newPage();
  const title = `Conflict ${Date.now()}`;
  try {
    await connect(first, 'conflict-one');
    await connect(second, 'conflict-two');
    await createNote(first, title, 'shared base');
    await expect(second.getByText(title, { exact: true })).toBeVisible({ timeout: 45_000 });
    await first.getByText(title, { exact: true }).click();
    await second.getByText(title, { exact: true }).click();
    await firstContext.setOffline(true);
    await secondContext.setOffline(true);
    for (const [page, body] of [[first, 'first offline branch'], [second, 'second offline branch']] as const) {
      await page.getByRole('button', { name: 'Edit' }).click();
      const editor = page.getByRole('textbox', { name: 'Frontmatter and Markdown' });
      const markdown = await editor.inputValue();
      await editor.fill(markdown.replace(/(---[\s\S]*?---)[\s\S]*/, `$1\n${body}`));
      await page.getByRole('button', { name: 'Save note' }).click();
    }
    await firstContext.setOffline(false);
    await secondContext.setOffline(false);
    await expect(first.getByText('conflict', { exact: true })).toBeVisible({ timeout: 45_000 });
    await expect(second.getByText('conflict', { exact: true })).toBeVisible({ timeout: 45_000 });
  } finally {
    await firstContext.setOffline(false);
    await secondContext.setOffline(false);
    await firstContext.close();
    await secondContext.close();
  }
});

test('wipes the browser replica and returns to client onboarding', async ({ page }) => {
  await connect(page, 'wipe-browser');
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  page.once('dialog', (dialog) => dialog.accept());
  await page.getByRole('button', { name: 'Wipe all browser data' }).click();
  await expect(page.getByLabel('Client ID')).toBeVisible();
});

test('prevents mobile focus zoom and horizontal overflow', async ({ page }) => {
  await page.goto('/');
  const viewport = await page.locator('meta[name="viewport"]').getAttribute('content');
  expect(viewport).toContain('width=device-width');
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);
});
