import { expect, test } from '@playwright/test';

const nativeTicket = process.env.XO_IROH_TICKET;

test('creates a relay-backed Iroh document and recovers an offline write', async ({ page, context, request }) => {
  const consoleErrors: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });

  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await expect(page.getByText('relay-only E2EE')).toBeVisible();
  const versionResponse = await request.get('/version.json');
  const deployed = await versionResponse.json() as { version: string };
  await expect(page.locator('.app-footer')).toHaveText(`xo ${deployed.version}`);
  await expect(page.locator('.runtime-card')).toContainText(`xo-web ${deployed.version}`);
  await expect(page.getByText('A newer xo release is available.')).toHaveCount(0);

  await page.getByRole('button', { name: 'Create workspace' }).click();
  await expect(page.getByText('Workspace online.')).toBeVisible();
  await expect(page.getByText('Iroh document connected')).toBeVisible();
  await page.getByRole('button', { name: 'Reveal ticket' }).click();
  const ticket = await page.locator('.ticket-output').inputValue();
  const serializedVault = await page.evaluate(async () => {
    const database = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open('xo-web');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const transaction = database.transaction('vault', 'readonly');
    const records = await new Promise<unknown[]>((resolve, reject) => {
      const request = transaction.objectStore('vault').getAll();
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    database.close();
    return JSON.stringify(records);
  });
  expect(serializedVault).not.toContain(ticket);

  await page.getByLabel('Key').fill('web/playwright');
  await page.getByLabel('UTF-8 value').fill('survives browser recovery');
  await page.getByRole('button', { name: 'Commit to Iroh Docs' }).click();
  await expect(page.locator('.entry-list').getByText('web/playwright')).toBeVisible();
  await expect(page.locator('.entry-list').getByText('survives browser recovery')).toBeVisible();

  await page.evaluate(() => navigator.serviceWorker.ready);
  await context.setOffline(true);
  await page.reload();
  await expect(page.getByText('Workspace online.')).toBeVisible();
  await expect(page.locator('.entry-list').getByText('web/playwright')).toBeVisible();
  await expect(page.locator('.entry-list').getByText('survives browser recovery')).toBeVisible();
  await expect(page.getByText(/Cached entries and pending writes remain available offline/)).toBeVisible();

  expect(consoleErrors.filter((message) => !message.includes('net::ERR_INTERNET_DISCONNECTED'))).toEqual([]);
});

test('offers a full refresh when the deployed version changes', async ({ page }) => {
  await page.route('**/version.json*', async (route) => {
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({ version: '20991231T235959Z' }),
    });
  });
  await page.goto('/');
  await expect(page.getByText('A newer xo release is available.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Refresh full app' })).toBeVisible();
});

test('checks the deployed version after a service-worker cached reload', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => navigator.serviceWorker.ready);
  await page.route('**/version.json*', async (route) => {
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({ version: '20991231T235959Z' }),
    });
  });
  await page.reload();
  await expect(page.getByText('A newer xo release is available.')).toBeVisible();
});

test('checks the deployed version every ten minutes', async ({ page, request }) => {
  const current = await (await request.get('/version.json')).json() as { version: string };
  let deployedVersion = current.version;
  await page.route('**/version.json*', async (route) => {
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({ version: deployedVersion }),
    });
  });
  await page.clock.install();
  await page.goto('/');
  await expect(page.getByText('A newer xo release is available.')).toHaveCount(0);
  deployedVersion = '20991231T235959Z';
  await page.clock.fastForward(10 * 60 * 1_000);
  await expect(page.getByText('A newer xo release is available.')).toBeVisible();
});

test('converges two browser peers through a native Iroh document peer', async ({ browser }) => {
  test.skip(!nativeTicket, 'XO_IROH_TICKET is required for the networked convergence test');
  const firstContext = await browser.newContext();
  const secondContext = await browser.newContext();
  const first = await firstContext.newPage();
  const second = await secondContext.newPage();
  const key = `web/convergence-${Date.now()}`;
  const value = 'browser peers converged through native Iroh';

  try {
    await first.goto('/');
    await expect(first.getByText('Runtime ready')).toBeVisible();
    await first.getByLabel('Writable workspace ticket').fill(nativeTicket!);
    await first.getByRole('button', { name: 'Join and synchronize' }).click();
    await expect(first.getByText('Workspace online.')).toBeVisible();
    await first.getByLabel('Key').fill(key);
    await first.getByLabel('UTF-8 value').fill(value);
    await first.getByRole('button', { name: 'Commit to Iroh Docs' }).click();
    await expect(first.locator('.entry-list').getByText(key)).toBeVisible();

    await second.goto(`/#ticket=${encodeURIComponent(nativeTicket!)}`);
    await expect(second.getByText('Workspace online.')).toBeVisible();
    await expect(second).toHaveURL(/\/$/);
    const replicated = second.locator('.entry-row').filter({ hasText: key });
    await expect(replicated).toBeVisible({ timeout: 60_000 });
    await expect(replicated).toContainText(value);
  } finally {
    await firstContext.close();
    await secondContext.close();
  }
});
