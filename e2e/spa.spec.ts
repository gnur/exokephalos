import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const password = process.env.EXO_E2E_PASSWORD;
const clientID = process.env.EXO_E2E_CLIENT_ID ?? 'e2e-tui-client';

test.skip(!password, 'EXO_E2E_PASSWORD is required; run through task test:e2e');

test('SPA login, mobile shell, editor, approval, and browser outbox', async ({ page, browserName, request }) => {
  await login(page);
  await expect(page.locator('.app-shell')).toBeVisible();
  await expect(page.locator('.topbar').getByRole('heading', { name: 'Items' })).toBeVisible();
  await expect(page.getByRole('search')).toBeVisible();
  await expect(page.getByRole('button', { name: 'New item' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Menu' })).toBeVisible();

  const horizontalOverflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
  expect(horizontalOverflow).toBe(false);

  await approvePendingClient(page);
  await verifyBottomLeftViewsMenu(page);
  await expect(page).toHaveURL(/\/views\/notes/);
  await expect(page.locator('.item-row').first()).toBeVisible({ timeout: 20_000 });
  await expect(page.locator('.pane-tabs')).toHaveCount(0);
  await page.getByRole('button', { name: 'Actions' }).click();
  await expect(page.getByRole('button', { name: 'Import from hardcover' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Edit', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Actions' }).click();
  await exerciseTagFiltering(page);

  await page.locator('.item-row').first().click();
  await expect(page).toHaveURL(/\/views\/notes\/[^/?]+/);
  await expect(page.locator('.markdown-body h1, .markdown-body p').first()).toBeVisible();
  await expect(page.locator('.frontmatter-view')).toContainText('type:');

  await page.getByRole('button', { name: 'Actions' }).click();
  await expect(page.getByRole('button', { name: 'Edit', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Mark item as done' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start reading this book' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByLabel('Raw markdown')).toContainText('---');
  await page.getByRole('button', { name: 'Cancel' }).click();

  await page.getByRole('button', { name: 'New item' }).click();
  const onlineTitle = `SPA E2E ${Date.now()}`;
  await page.locator('label').filter({ hasText: 'Title' }).locator('input').fill(onlineTitle);
  await page.locator('label').filter({ hasText: 'Body' }).locator('textarea').fill('Created from the SPA E2E browser.');
  await page.getByRole('button', { name: 'Create' }).click();
  await expect.poll(() => itemIDByTitle(page, onlineTitle), { timeout: 10_000 }).toMatch(exoIDPatternForToday());
  const onlineID = await itemIDByTitle(page, onlineTitle);
  await expect.poll(() => pendingBrowserOutboxCount(page), { timeout: 10_000 }).toBe(0);
  await exerciseEncryptedNote(page);
  await exerciseAPIKeyManagement(page, request, onlineID);
  await exerciseConfigSettings(page);

  await page.context().setOffline(true);
  await expect(page.locator('.sync-warning')).toContainText('sync offline');
  await page.getByRole('button', { name: 'New item' }).click();
  const offlineTitle = `Offline SPA E2E ${Date.now()}`;
  await page.locator('label').filter({ hasText: 'Title' }).locator('input').fill(offlineTitle);
  await page.locator('label').filter({ hasText: 'Body' }).locator('textarea').fill(`Offline browser change from ${browserName}.`);
  await page.getByRole('button', { name: 'Create' }).click();
  await expect.poll(() => itemIDByTitle(page, offlineTitle), { timeout: 10_000 }).toMatch(exoIDPatternForToday());
  await expect.poll(() => pendingBrowserOutboxCount(page), { timeout: 10_000 }).toBeGreaterThan(0);
  await page.context().setOffline(false);
  await expect.poll(() => pendingBrowserOutboxCount(page), { timeout: 20_000 }).toBe(0);
  await expect(page.locator('.sync-warning')).toHaveCount(0);
});

async function login(page: Page) {
  await page.goto('/');
  if (page.url().includes('/login')) {
    await page.locator('input[name="password"]').fill(password!);
    await page.getByRole('button', { name: 'Log in' }).click();
  }
  await page.waitForURL((url) => !url.pathname.startsWith('/login'));
}

async function exerciseEncryptedNote(page: Page) {
  await page.getByRole('button', { name: 'New item' }).click();
  const title = `Encrypted SPA E2E ${Date.now()}`;
  const secret = 'encrypted browser body';
  await page.locator('label').filter({ hasText: 'Title' }).locator('input').fill(title);
  await page.locator('label').filter({ hasText: 'Body' }).locator('textarea').fill(secret);
  await page.getByLabel('Encrypt body').check();
  page.once('dialog', dialog => dialog.accept('playwright-passphrase'));
  await page.getByRole('button', { name: 'Create' }).click();
  await expect.poll(() => itemIDByTitle(page, title), { timeout: 10_000 }).toMatch(exoIDPatternForToday());
  const id = await itemIDByTitle(page, title);
  await page.evaluate((noteID) => {
    history.pushState(null, '', `/views/notes/${noteID}?pane=editor`);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, id);
  await expect(page.getByText('This note body is encrypted.')).toBeVisible();
  await expect(page.locator('.markdown-body')).toHaveCount(0);
  page.once('dialog', dialog => dialog.accept('playwright-passphrase'));
  await page.getByRole('button', { name: 'Unlock' }).click();
  await expect(page.locator('.markdown-body')).toContainText(secret);
  await page.getByRole('button', { name: 'Actions' }).click();
  page.once('dialog', dialog => dialog.accept('playwright-passphrase'));
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByLabel('Raw markdown')).toContainText(secret);
  await page.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByText('This note body is encrypted.')).toBeVisible();
}

async function approvePendingClient(page: Page) {
  await page.getByRole('button', { name: 'Menu' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('button', { name: 'Sync clients' }).click();
  await expect(page.getByRole('heading', { name: 'Sync clients' })).toBeVisible();
  const row = page.locator('.outbox-row').filter({ hasText: clientID });
  await expect(row).toContainText('pending');
  await row.getByRole('button', { name: 'Approve' }).click();
  await expect(row).toContainText('approved');
  await expect(row.getByRole('button', { name: 'Approve' })).toHaveCount(0);
}

async function exerciseAPIKeyManagement(page: Page, request: APIRequestContext, matchingItemID: string) {
  await page.getByRole('button', { name: 'Menu' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('button', { name: 'API keys' }).click();
  await page.locator('label').filter({ hasText: 'App name' }).locator('input').fill(`E2E API ${Date.now()}`);
  await page.locator('label').filter({ hasText: 'CEL filter' }).locator('textarea').fill('type == "note"');
  await page.getByRole('button', { name: 'Create API key' }).click();
  const key = await expect.poll(async () => {
    const text = await page.locator('.notice code').last().textContent();
    return text?.trim() ?? '';
  }, { timeout: 10_000 }).toMatch(/^exo_[0-9A-Za-z]+$/).then(async () => (await page.locator('.notice code').last().textContent())!.trim());

  const ok = await request.get(`/api/items/${matchingItemID}`, {
    headers: { Authorization: `Bearer ${key}` },
  });
  expect(ok.status()).toBe(200);

  const hidden = await request.get('/api/items/e2edoc', {
    headers: { 'X-API-Key': key },
  });
  expect(hidden.status()).toBe(404);

  await page.reload();
  await page.getByRole('button', { name: 'Menu' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('button', { name: 'API keys' }).click();
  const row = page.locator('.outbox-row').filter({ hasText: 'E2E API' });
  await expect(row).toContainText('last used');
  await row.getByRole('button', { name: 'Revoke' }).click();
  await expect(row).toContainText('revoked');

  const revoked = await request.get(`/api/items/${matchingItemID}`, {
    headers: { Authorization: `Bearer ${key}` },
  });
  expect(revoked.status()).toBe(401);
}

async function verifyBottomLeftViewsMenu(page: Page) {
  const menuButton = page.getByRole('button', { name: 'Menu' });
  const box = await menuButton.boundingBox();
  const viewport = page.viewportSize();
  expect(box).not.toBeNull();
  expect(viewport).not.toBeNull();
  expect(box!.x).toBeLessThan(80);
  expect(box!.y).toBeGreaterThan(viewport!.height - 90);

  await menuButton.click();
  const panel = page.locator('.menu-panel');
  await expect(panel).toBeVisible();
  await expect(panel.getByRole('button', { name: 'Notes', exact: true })).toBeVisible();
  await expect(panel.getByRole('button', { name: 'Docs', exact: true })).toBeVisible();
  await expect(panel.getByRole('button', { name: 'All', exact: true }).first()).toBeVisible();
  await panel.getByRole('button', { name: 'Notes', exact: true }).click();
  await expect(panel).toBeVisible();
  await expect(page).not.toHaveURL(/\/views\/notes/);
  await panel.locator('.menu-section.subviews').getByRole('button', { name: 'All', exact: true }).click();
  await expect(panel).toHaveCount(0);
}

async function exerciseConfigSettings(page: Page) {
  await page.getByRole('button', { name: 'Menu' }).click();
  await page.locator('.menu-panel').getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Fennel/Lua settings' }).click();
  await expect(page.getByRole('heading', { name: 'Fennel/Lua settings' })).toBeVisible();
  await page.locator('label').filter({ hasText: 'Config file' }).locator('select').selectOption('exo.fnl');
  const editor = page.getByLabel('Fennel/Lua settings');
  await expect(editor).toContainText(':views');
  await page.getByRole('button', { name: 'Save Fennel/Lua settings' }).click();
  await expect(page.locator('.notice').filter({ hasText: 'Fennel/Lua settings saved' })).toBeVisible();
}

async function exerciseTagFiltering(page: Page) {
  const tagsButton = page.getByRole('button', { name: /^Tags/ });
  await expect(tagsButton).toBeVisible();
  await tagsButton.click();
  await expect(page).toHaveURL(/pane=tags/);
  const firstTag = page.locator('.tag-row').first();
  await expect(firstTag).toBeVisible();
  const firstCount = Number(await firstTag.locator('strong').textContent());
  expect(firstCount).toBeGreaterThan(0);
  await firstTag.click();
  await expect(page).toHaveURL(/tags=/);
  await expect(firstTag).toHaveClass(/active/);
  await expect.poll(async () => {
    const rows = await page.locator('.tag-row strong').allTextContents();
    return rows.every((value) => Number(value) <= firstCount);
  }).toBe(true);
  await page.getByRole('button', { name: 'View results' }).click();
  await expect(page).toHaveURL(/tags=/);
  await expect(page.locator('.item-row').first()).toBeVisible();
}

async function pendingBrowserOutboxCount(page: Page) {
  return page.evaluate(async () => {
    return new Promise<number>((resolve, reject) => {
      const open = indexedDB.open('exokephalos');
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const database = open.result;
        const tx = database.transaction('outbox', 'readonly');
        const store = tx.objectStore('outbox');
        const req = store.getAll();
        req.onerror = () => reject(req.error);
        req.onsuccess = () => {
          const rows = req.result as Array<{ status?: string }>;
          database.close();
          resolve(rows.filter((row) => row.status === 'pending' || row.status === 'failed' || row.status === 'syncing').length);
        };
      };
    });
  });
}

async function itemIDByTitle(page: Page, title: string) {
  return page.evaluate(async (itemTitle) => {
    return new Promise<string>((resolve, reject) => {
      const open = indexedDB.open('exokephalos');
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const database = open.result;
        const tx = database.transaction('items', 'readonly');
        const store = tx.objectStore('items');
        const req = store.getAll();
        req.onerror = () => reject(req.error);
        req.onsuccess = () => {
          const rows = req.result as Array<{ id?: string; title?: string; frontmatter?: { title?: unknown } }>;
          database.close();
          const item = rows.find((row) => row.title === itemTitle || row.frontmatter?.title === itemTitle);
          resolve(item?.id ?? '');
        };
      };
    });
  }, title);
}

function exoIDPatternForToday() {
  const prefix = encodeBase32(daysSinceExoEpoch(new Date()));
  return new RegExp(`^${prefix}[a-z2-7]{4}$`);
}

function daysSinceExoEpoch(date: Date) {
  const epoch = Date.UTC(1989, 0, 17);
  return Math.max(0, Math.floor((date.getTime() - epoch) / 86_400_000));
}

function encodeBase32(value: number) {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz234567';
  if (value === 0) return 'a';
  let result = '';
  let n = value;
  while (n > 0) {
    result = alphabet[n % 32] + result;
    n = Math.floor(n / 32);
  }
  return result;
}
