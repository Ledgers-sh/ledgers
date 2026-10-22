import { defineConfig, devices } from '@playwright/test';

/** Port of the app under test. LEDGERS_E2E_PORT moves it when another local server holds 5173. */
const port = Number(process.env.LEDGERS_E2E_PORT ?? 5173);

export default defineConfig({
  testDir: './tests/e2e',
  // One worker: the fake API listens on 127.0.0.1:3000 and is shared by the tests.
  workers: 1,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [['list']],
  use: {
    baseURL: `http://localhost:${port}`,
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } },
  ],
  webServer: {
    command: `npm run build && npx vite preview --port ${port} --strictPort`,
    url: `http://localhost:${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
