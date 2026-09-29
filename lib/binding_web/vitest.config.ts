import { configDefaults, defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    exclude: [...configDefaults.exclude, 'test/browser/**'],
    coverage: {
      include: [
        'web-tree-sitter.js',
      ],
      exclude: [
        'test/**',
        'dist/**',
        'lib/**',
        'wasm/**'
      ],
    },
  }
})
