import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: process.env.EXO_E2E_BASE_URL || 'http://127.0.0.1:8293',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    {
      name: 'approve-and-exercise-spa',
      use: {
        browserName: 'chromium',
        viewport: { width: 393, height: 852 },
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 3,
        userAgent:
          'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1',
      },
      testMatch: /spa\.spec\.ts/,
    },
    {
      name: 'android-spa',
      use: { ...devices['Pixel 7'] },
      testMatch: /spa\.spec\.ts/,
    },
    {
      name: 'desktop-spa',
      use: { ...devices['Desktop Chrome'] },
      testMatch: /spa\.spec\.ts/,
    },
  ],
});
