import { defineConfig } from 'vitest/config';

export default defineConfig({
  oxc: { jsx: { runtime: 'automatic', importSource: 'preact' } },
  server: { port: 5173, strictPort: true },
  preview: { port: 5173, strictPort: true },
  build: { target: 'es2022', sourcemap: false },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
