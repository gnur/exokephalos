import { expect, test, type Page } from '@playwright/test';

const password = process.env.EXO_E2E_PASSWORD;
const clientID = process.env.EXO_E2E_CLIENT_ID ?? 'e2e-tui-client';

test.skip(!password, 'EXO_E2E_PASSWORD is required; run through task test:e2e');

test('SPA login, mobile shell, editor, approval, and browser outbox', async ({ page, browserName }) => {
  await login(page);
  await expect(page.locator('.app-shell')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Items' })).toBeVisible();
  await expect(page.getByRole('search')).toBeVisible();
  await expect(page.getByRole('button', { name: 'New item' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Menu' })).toBeVisible();

  const horizontalOverflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
  expect(horizontalOverflow).toBe(false);

  await approvePendingClient(page);
  await page.getByRole('button', { name: 'Menu' }).click();
  await expect(page.getByRole('button', { name: 'Notes' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'All' }).first()).toBeVisible();
  await page.getByRole('button', { name: 'Notes' }).click();
  await expect(page).toHaveURL(/\/views\/notes/);
  await expect(page.locator('.item-row').first()).toBeVisible({ timeout: 20_000 });

  await page.locator('.item-row').first().click();
  await expect(page).toHaveURL(/\/views\/notes\/[^/?]+/);
  await expect(page.locator('.markdown-body h1, .markdown-body p').first()).toBeVisible();
  await expect(page.locator('.frontmatter-view')).toContainText('type:');

  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByLabel('Raw markdown')).toContainText('---');
  await page.getByRole('button', { name: 'Cancel' }).click();

  await page.getByRole('button', { name: 'New item' }).click();
  const onlineTitle = `SPA E2E ${Date.now()}`;
  await page.locator('label').filter({ hasText: 'Title' }).locator('input').fill(onlineTitle);
  await page.locator('label').filter({ hasText: 'Body' }).locator('textarea').fill('Created from the SPA E2E browser.');
  await page.getByRole('button', { name: 'Create' }).click();
  await expect.poll(() => itemIDByTitle(page, onlineTitle), { timeout: 10_000 }).toMatch(exoIDPatternForToday());
  await expect.poll(() => pendingBrowserOutboxCount(page), { timeout: 10_000 }).toBe(0);

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

async function approvePendingClient(page: Page) {
  await page.getByRole('button', { name: 'Menu' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Sync clients' })).toBeVisible();
  const row = page.locator('.outbox-row').filter({ hasText: clientID });
  await expect(row).toContainText('pending');
  await row.getByRole('button', { name: 'Approve' }).click();
  await expect(row).toContainText('approved');
  await expect(row.getByRole('button', { name: 'Approve' })).toHaveCount(0);
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
