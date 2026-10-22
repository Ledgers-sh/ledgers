import { defineConfig, devices } from '@playwright/test';

const webPort = Number(process.env.E2E_WEB_PORT ?? 5173);
const apiPort = Number(process.env.E2E_API_PORT ?? 3000);

export default defineConfig({
  testDir: './tests/e2e',
  // One worker: the fake API listens on 127.0.0.1:E2E_API_PORT (default 3000) and is shared by the tests.
  workers: 1,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [['list']],
  use: {
    baseURL: `http://localhost:${webPort}`,
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } },
  ],
  webServer: {
    // The build points the sign-in screen at this run's fake API, not the default port.
    command: `VITE_LEDGERS_API=http://127.0.0.1:${apiPort} npm run build && npx vite preview --port ${webPort} --strictPort`,
    url: `http://localhost:${webPort}`,
    // A preview started for another checkout is never reused: it would serve that build.
    reuseExistingServer: !process.env.CI && !process.env.E2E_WEB_PORT,
    timeout: 120_000,
  },
});
