import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

const nativeTicket = process.env.XO_IROH_TICKET;
const operatorToken = process.env.XO_OPERATOR_TOKEN;
const tuiApprovalUrl = process.env.XO_TUI_APPROVAL_URL;

async function ensureAutomaticAdmission(page: Page) {
  const newNote = page.getByRole('button', { name: 'New note' });
  try {
    await expect(newNote).toBeVisible({ timeout: 5_000 });
    return;
  } catch {
    // Compatibility fallback below.
  }
  // Confirm an approval that may have been recorded during a first-round
  // transport error before attempting the legacy manual fallback.
  const admission = page.getByRole('button', { name: 'Check admission' });
  if (await admission.isVisible()) await admission.click();
  else await page.getByRole('button', { name: 'Join and synchronize' }).click();
  try {
    await expect(newNote).toBeVisible({ timeout: 5_000 });
    return;
  } catch {
    // Compatibility fallback for an active peer still using manual admission.
  }
  if (tuiApprovalUrl) {
    const response = await fetch(tuiApprovalUrl, { method: 'POST' });
    if (response.ok) await response.json();
  } else if (operatorToken) {
    const response = await fetch('http://127.0.0.1:19464/v1/members/approve-pending', {
      method: 'POST',
      headers: { authorization: `Bearer ${operatorToken}` },
    });
    if (!response.ok) throw new Error(`membership fallback failed: ${response.status}`);
    await response.json(); // zero means the peer already auto-approved the request
  } else {
    throw new Error('automatic admission did not complete');
  }
  if (await admission.isVisible()) {
    await admission.click();
  } else {
    // A first-round network error may leave the invitation form visible even
    // though the active peer durably recorded the automatic admission.
    await page.getByRole('button', { name: 'Join and synchronize' }).click();
  }
  await expect(newNote).toBeVisible({ timeout: 120_000 });
}

async function configurePeer(page: Page, peerId: string) {
  const input = page.getByLabel('Peer ID');
  if (await input.isVisible()) await input.fill(peerId);
}

async function selectWorkspaceNavigation(page: Page, name: string) {
  const button = page.getByRole('navigation', { name: 'Workspace' }).getByRole('button', { name, exact: true });
  const menu = page.getByRole('button', { name: 'Open navigation' });
  if (await menu.isVisible()) await menu.click();
  await button.click();
}

test('creates a relay-backed Iroh document and recovers an offline write', async ({ page, context, request }) => {
  const consoleErrors: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });

  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await expect(page.getByText('A Peer ID is required before joining.')).toBeVisible();
  await expect(page.getByText('A random client name has been generated for this browser. You can change it before creating or joining a workspace.')).toBeVisible();
  await expect(page.getByLabel('Peer ID')).toHaveValue(/^(smart|clever|funny|incredible|blue|green)-(xo|exokephalos|zettelkasten|sandbox|browser|client)$/);
  await configurePeer(page, 'playwright-primary');
  await expect(page.getByText('relay-only E2EE')).toBeVisible();
  const versionResponse = await request.get('/version.json');
  const deployed = await versionResponse.json() as { version: string };
  await expect(page.locator('.app-footer')).toHaveText(`xo ${deployed.version}`);
  await expect(page.locator('.runtime-card')).toContainText(`xo-web ${deployed.version}`);
  await expect(page.getByText('A newer xo release is available.')).toHaveCount(0);

  await page.getByRole('button', { name: 'Create workspace' }).click();
  await expect(page.locator('.notes-toolbar').getByRole('heading', { name: 'Notes' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New note' })).toBeVisible();
  await expect(page.locator('.bottom-search')).toBeVisible();
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Reveal ticket' }).click();
  const ticket = await page.locator('.ticket-output').inputValue();
  await selectWorkspaceNavigation(page, 'Notes');
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
  await expect(page.locator('.notes-toolbar')).toBeVisible();
  await expect(page.getByText('Web Playwright', { exact: true }).first()).toBeVisible();
  await expect(page).toHaveURL(/\/views\/notes(?:\?|$)/);
  await expect(page.getByRole('button', { name: 'Update', exact: true })).toHaveCount(0);
  await page.getByText('Web Playwright', { exact: true }).first().click();
  await expect(page.locator('.markdown-preview')).toContainText('survives browser recovery');
  await expect(page.locator('.frontmatter-grid')).toContainText('type');
  await expect(page.locator('.frontmatter-grid')).toContainText('note');
  const displayedCreated = page.locator('.frontmatter-grid div').filter({ hasText: 'created' }).locator('dd');
  await expect(displayedCreated).toHaveText(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/);
  await expect(displayedCreated).not.toContainText(/[+-]\d{2}:\d{2}/);
  await page.getByText(/Revision history/).click();
  await expect(page.locator('.history-panel')).not.toContainText(/[+-]\d{2}:\d{2}/);
  await page.getByRole('button', { name: 'Edit' }).click();
  await page.getByLabel('Frontmatter and Markdown').fill('---\ntitle: Web Playwright\ntype: note\ntags: [browser, edited]\n---\nedited and survives browser recovery');
  await page.getByRole('button', { name: 'Save note' }).click();
  await expect(page.locator('.notes-toolbar')).toBeVisible();
  await page.getByText('Web Playwright', { exact: true }).first().click();
  await expect(page.locator('.markdown-preview')).toContainText('edited and survives browser recovery');
  await selectWorkspaceNavigation(page, 'All');
  await expect(page.getByRole('heading', { name: String(new Date().getFullYear()) })).toBeVisible();
  await expect(page.getByText('Web Playwright', { exact: true }).first()).toBeVisible();
  await selectWorkspaceNavigation(page, 'Notes');
  await page.getByText('Web Playwright', { exact: true }).first().click();
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
  await expect(page.getByText('Web Playwright', { exact: true }).first()).toBeVisible();
  await page.getByText('Web Playwright', { exact: true }).first().click();
  await expect(page.locator('.markdown-preview')).toContainText('edited and survives browser recovery');
  await page.getByRole('button', { name: 'Items' }).click();
  await page.getByRole('button', { name: 'New note' }).click();
  await page.getByLabel('Title', { exact: true }).fill('Offline creation');
  await page.getByRole('button', { name: 'Save note' }).click();
  await expect(page.getByText('Offline creation', { exact: true })).toBeVisible();
  await expect(page.getByText(/saved change.*waiting to sync/)).toBeVisible();

  expect(consoleErrors.filter((message) => !message.includes('net::ERR_INTERNET_DISCONNECTED'))).toEqual([]);
});

test('wipes the browser client from settings and returns to onboarding', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await configurePeer(page, 'playwright-wipe');
  await page.getByRole('button', { name: 'Create workspace' }).click();
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Wipe all browser data' })).toBeVisible();
  page.once('dialog', (dialog) => dialog.accept());
  await page.getByRole('button', { name: 'Wipe all browser data' }).click();
  await expect(page.getByText('Runtime ready')).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole('button', { name: 'Create workspace' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New note' })).toHaveCount(0);
});

test('creates a local item without attempting to synchronize with itself', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await configurePeer(page, 'playwright-local');
  await page.getByRole('button', { name: 'Create workspace' }).click();
  await page.getByRole('button', { name: 'New note' }).click();
  await page.getByLabel('Title', { exact: true }).fill('Local item');
  await page.getByRole('button', { name: 'Save note' }).click();
  await expect(page.getByText('Local item', { exact: true }).first()).toBeVisible();
  await page.getByText('Local item', { exact: true }).first().click();
  await expect(page.locator('.frontmatter-grid')).toContainText('note');
  await page.waitForTimeout(3_500);
  await expect(page.getByText(/initial document sync failed/i)).toHaveCount(0);
});

test('prevents mobile focus zoom and horizontal page overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await configurePeer(page, 'playwright-mobile');
  await expect(page.locator('meta[name="viewport"]')).toHaveAttribute('content', /maximum-scale=1, user-scalable=no/);
  await page.getByRole('button', { name: 'Create workspace' }).click();
  await page.getByRole('button', { name: 'New note' }).click();
  await page.getByLabel('Title', { exact: true }).focus();
  await page.getByLabel('Frontmatter and Markdown').focus();
  const dimensions = await page.evaluate(() => ({ width: window.innerWidth, scrollWidth: document.documentElement.scrollWidth }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.width);
});

test('offers an update only when an existing workspace is running an outdated version', async ({ page }) => {
  let deployedVersion: string | undefined;
  await page.route('**/version.json*', async (route) => {
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({ version: deployedVersion }),
    });
  });
  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await configurePeer(page, 'playwright-update');
  await page.getByRole('button', { name: 'Create workspace' }).click();
  await page.getByRole('button', { name: 'New note' }).click();
  await page.getByLabel('Title', { exact: true }).fill('Update fixture');
  await page.getByRole('button', { name: 'Save note' }).click();
  await expect(page.getByRole('button', { name: 'Update', exact: true })).toHaveCount(0);

  deployedVersion = '20991231T235959Z';
  await page.evaluate(() => window.dispatchEvent(new Event('pageshow')));
  await expect(page.getByText('A newer xo release is available.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Update', exact: true })).toBeVisible();
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

test('imports notes, starts the actual TUI, synchronizes to the PWA, and survives refresh', async ({ page }) => {
  test.setTimeout(240_000);
  test.skip(!nativeTicket, 'the native TUI fixture is required');
  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await configurePeer(page, 'playwright-tui-flow');
  await page.getByLabel('Workspace invitation').fill(nativeTicket!);
  await page.getByRole('button', { name: 'Join and synchronize' }).click();
  await ensureAutomaticAdmission(page);
  await selectWorkspaceNavigation(page, 'Notes');
  const imported = page.locator('.note-list-item').filter({ hasText: 'Browser fixture' });
  await expect(imported).toBeVisible({ timeout: 60_000 });
  await imported.click();
  await expect(page.locator('.markdown-preview')).toContainText('created by a native peer');

  await page.reload();
  await expect(page.locator('.markdown-preview')).toContainText('created by a native peer', { timeout: 10_000 });
  await expect(page.getByRole('button', { name: 'Join and synchronize' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New note' })).toBeEnabled({ timeout: 120_000 });
  await expect(page.getByText('Runtime unavailable')).toHaveCount(0);
  await expect(page.getByText('unreachable executed')).toHaveCount(0);
  await selectWorkspaceNavigation(page, 'Notes');
  await expect(page.getByText('Browser fixture', { exact: true }).first()).toBeVisible();
  await page.getByText('Browser fixture', { exact: true }).first().click();
  await expect(page.locator('.markdown-preview')).toContainText('created by a native peer');
});

test('receives native items and replicated views and subviews', async ({ page, browserName }) => {
  test.setTimeout(180_000);
  test.skip(!nativeTicket, 'XO_IROH_TICKET is required for the networked convergence test');
  await page.goto('/');
  await expect(page.getByText('Runtime ready')).toBeVisible();
  await configurePeer(page, `playwright-native-${browserName}`);
  await page.getByLabel('Workspace invitation').fill(nativeTicket!);
  await page.getByRole('button', { name: 'Join and synchronize' }).click();
  await ensureAutomaticAdmission(page);

  await page.getByRole('button', { name: 'Open navigation' }).click();
  const workspaceNavigation = page.getByRole('navigation', { name: 'Workspace' });
  await expect(workspaceNavigation.getByRole('button', { name: 'Library' })).toBeVisible({ timeout: 60_000 });
  await expect(workspaceNavigation.getByRole('button', { name: 'Reading' })).toBeVisible();
  await selectWorkspaceNavigation(page, 'Library');
  await expect(page.getByText('TUI Reading Fixture', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('TUI Finished Fixture', { exact: true }).first()).toBeVisible();
  await selectWorkspaceNavigation(page, 'Reading');
  await expect(page.getByText('TUI Reading Fixture', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('TUI Finished Fixture', { exact: true })).toHaveCount(0);
  await selectWorkspaceNavigation(page, 'Notes');
  const fixtureRow = page.locator('.note-list-item').filter({ hasText: 'Browser fixture' });
  await expect(fixtureRow).toBeVisible();
  await fixtureRow.click();
  await expect(page.locator('.markdown-preview')).toContainText('created by a native peer');
});

test('converges two browser peers through a native Iroh document peer', async ({ browser }) => {
  test.setTimeout(240_000);
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
    await configurePeer(first, 'playwright-browser-one');
    await first.getByLabel('Workspace invitation').fill(nativeTicket!);
    await first.getByRole('button', { name: 'Join and synchronize' }).click();
    await ensureAutomaticAdmission(first);
    await selectWorkspaceNavigation(first, 'Notes');

    await first.getByRole('button', { name: 'New note' }).click();
    await first.getByLabel('Title', { exact: true }).fill(title);
    await first.getByLabel('Frontmatter and Markdown').fill(`---\ntitle: ${title}\ntype: note\ntags: [browser]\n---\n${value}`);
    await first.getByRole('button', { name: 'Save note' }).click();
    await expect(first.getByText(title, { exact: true }).first()).toBeVisible();

    await second.goto(`/#ticket=${encodeURIComponent(nativeTicket!)}`);
    await expect(second.getByText('Runtime ready')).toBeVisible();
    await configurePeer(second, 'playwright-browser-two');
    await second.getByRole('button', { name: 'Join and synchronize' }).click();
    await ensureAutomaticAdmission(second);
    await expect(second).toHaveURL(/\/views\//);
    expect(new URL(second.url()).hash).toBe('');
    await selectWorkspaceNavigation(second, 'Notes');
    await expect(second.getByText(title, { exact: true }).first()).toBeVisible({ timeout: 60_000 });
    await second.getByText(title, { exact: true }).first().click();
    await expect(second.locator('.markdown-preview')).toContainText(value);
  } finally {
    await firstContext.close();
    await secondContext.close();
  }
});
