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
  await expect(page.locator('.notes-toolbar').getByRole('heading', { name: 'Notes' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New note' })).toBeVisible();
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

  await page.getByRole('button', { name: 'New note' }).click();
  await expect(page.getByLabel('Frontmatter and Markdown')).toHaveValue(/\ntype: \ntags:/);
  await page.getByLabel('Title', { exact: true }).fill('Web Playwright');
  await page.getByLabel('Frontmatter and Markdown').fill('---\ntitle: Web Playwright\ntype: \ntags: [browser, test]\n---\nsurvives browser recovery');
  await page.getByRole('button', { name: 'Save note' }).click();
  await expect(page.getByText('Web Playwright', { exact: true }).first()).toBeVisible();
  await expect(page.locator('.markdown-preview')).toContainText('survives browser recovery');
  await expect(page.locator('.frontmatter-grid')).toContainText('type');
  await expect(page.locator('.frontmatter-grid')).toContainText('note');
  await page.getByRole('button', { name: 'Edit' }).click();
  await page.getByLabel('Frontmatter and Markdown').fill('---\ntitle: Web Playwright\ntype: note\ntags: [browser, edited]\n---\nedited and survives browser recovery');
  await page.getByRole('button', { name: 'Save note' }).click();
  await expect(page.locator('.markdown-preview')).toContainText('edited and survives browser recovery');
  await page.getByRole('button', { name: 'All', exact: true }).click();
  await expect(page.getByText('Web Playwright', { exact: true }).first()).toBeVisible();
  await page.getByRole('button', { name: 'Notes', exact: true }).click();
  page.once('dialog', (dialog) => dialog.accept());
  await page.getByRole('button', { name: 'Delete' }).click();
  await page.getByText('Deleted notes (1)').click();
  await page.getByRole('button', { name: 'Restore' }).click();
  await expect(page.getByText('Web Playwright', { exact: true }).first()).toBeVisible();
  await page.waitForTimeout(3_500);
  await expect(page.getByText(/initial document sync failed/i)).toHaveCount(0);

  await page.evaluate(() => navigator.serviceWorker.ready);
  await context.setOffline(true);
  await page.reload();
  await expect(page.locator('.notes-toolbar').getByRole('heading', { name: 'Notes' })).toBeVisible();
  await expect(page.getByText('Web Playwright', { exact: true }).first()).toBeVisible();
  await expect(page.locator('.markdown-preview')).toContainText('edited and survives browser recovery');

  expect(consoleErrors.filter((message) => !message.includes('net::ERR_INTERNET_DISCONNECTED'))).toEqual([]);
});

test('prevents mobile focus zoom and horizontal page overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await expect(page.locator('meta[name="viewport"]')).toHaveAttribute('content', /maximum-scale=1, user-scalable=no/);
  await page.getByRole('button', { name: 'Create workspace' }).click();
  await page.getByRole('button', { name: 'New note' }).click();
  await page.getByLabel('Title', { exact: true }).focus();
  await page.getByLabel('Frontmatter and Markdown').focus();
  const dimensions = await page.evaluate(() => ({ width: window.innerWidth, scrollWidth: document.documentElement.scrollWidth }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.width);
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

test('receives native items and replicated views and subviews', async ({ page }) => {
  test.skip(!nativeTicket, 'XO_IROH_TICKET is required for the networked convergence test');
  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await page.getByLabel('Writable workspace ticket').fill(nativeTicket!);
  await page.getByRole('button', { name: 'Join and synchronize' }).click();
  await expect(page.getByRole('button', { name: 'New note' })).toBeVisible();

  const workspaceNavigation = page.getByRole('navigation', { name: 'Workspace' });
  await expect(workspaceNavigation.getByRole('button', { name: 'Library' })).toBeVisible({ timeout: 60_000 });
  await expect(workspaceNavigation.getByRole('button', { name: 'Reading' })).toBeVisible();
  await workspaceNavigation.getByRole('button', { name: 'Library' }).click();
  await expect(page.getByText('TUI Reading Fixture', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('TUI Finished Fixture', { exact: true }).first()).toBeVisible();
  await workspaceNavigation.getByRole('button', { name: 'Reading' }).click();
  await expect(page.getByText('TUI Reading Fixture', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('TUI Finished Fixture', { exact: true })).toHaveCount(0);
  await workspaceNavigation.getByRole('button', { name: 'Notes' }).click();
  await expect(page.getByText('Browser fixture', { exact: true }).first()).toBeVisible();
  await expect(page.locator('.markdown-preview')).toContainText('created by a native peer');
});

test('converges two browser peers through a native Iroh document peer', async ({ browser }) => {
  test.skip(!nativeTicket, 'XO_IROH_TICKET is required for the networked convergence test');
  const firstContext = await browser.newContext();
  const secondContext = await browser.newContext();
  const first = await firstContext.newPage();
  const second = await secondContext.newPage();
  const title = `Browser convergence ${Date.now()}`;
  const value = 'browser peers converged through native Iroh';

  try {
    await first.goto('/');
    await expect(first.getByText('Runtime ready')).toBeVisible();
    await first.getByLabel('Writable workspace ticket').fill(nativeTicket!);
    await first.getByRole('button', { name: 'Join and synchronize' }).click();
    await expect(first.getByRole('button', { name: 'New note' })).toBeVisible();
    await first.getByRole('navigation', { name: 'Workspace' }).getByRole('button', { name: 'Notes' }).click();

    await first.getByRole('button', { name: 'New note' }).click();
    await first.getByLabel('Title', { exact: true }).fill(title);
    await first.getByLabel('Frontmatter and Markdown').fill(`---\ntitle: ${title}\ntype: note\ntags: [browser]\n---\n${value}`);
    await first.getByRole('button', { name: 'Save note' }).click();
    await expect(first.getByText(title, { exact: true }).first()).toBeVisible();

    await second.goto(`/#ticket=${encodeURIComponent(nativeTicket!)}`);
    await expect(second.getByRole('button', { name: 'New note' })).toBeVisible();
    await expect(second).toHaveURL(/\/$/);
    await second.getByRole('navigation', { name: 'Workspace' }).getByRole('button', { name: 'Notes' }).click();
    await expect(second.getByText(title, { exact: true }).first()).toBeVisible({ timeout: 60_000 });
    await expect(second.locator('.markdown-preview')).toContainText(value);
  } finally {
    await firstContext.close();
    await secondContext.close();
  }
});
