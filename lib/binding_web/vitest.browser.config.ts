import { defineConfig } from 'vitest/config';
import { playwright } from '@vitest/browser-playwright';

// Runs the tests under test/browser in Chromium, importing the package, so that its `browser` export condition selects the bundle.
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
      provider: playwright(),
      headless: true,
      instances: [{ browser: 'chromium' }],
    },
  },
});
