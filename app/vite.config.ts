import { defineConfig } from 'vitest/config';

/**
 * Local development. `/v1` (the API, and the people sign-in under `/v1/auth`)
 * is proxied to the local server so the browser sees one origin: the session
 * cookie and the sign-in redirect then work without credentialed CORS. Set
 * LEDGERS_DEV_API to point at another server (default: the native dev server).
 */
const devApi = process.env.LEDGERS_DEV_API ?? 'http://127.0.0.1:3000';

export default defineConfig({
  oxc: { jsx: { runtime: 'automatic', importSource: 'preact' } },
  server: { port: 5173, strictPort: true, proxy: { '/v1': { target: devApi, changeOrigin: false } } },
  preview: { port: 5173, strictPort: true },
  build: { target: 'es2022', sourcemap: false },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
