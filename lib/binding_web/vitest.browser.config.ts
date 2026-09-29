import { defineConfig } from 'vitest/config';

// Runs the tests under test/browser in Chromium, against the bundle that the `browser` export condition selects.
export default defineConfig({
  server: {
    fs: {
      // The fixture grammars are built into the repository's target directory.
      allow: ['../..'],
    },
  },
  test: {
    include: ['test/browser/**/*.test.ts'],
    browser: {
      enabled: true,
      provider: 'playwright',
      headless: true,
      instances: [{ browser: 'chromium' }],
    },
  },
});
