import eslint from '@eslint/js';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  // These files type-check against the generated declarations, which only `npm run build:dts` creates; CI checks them
  // with tsc and lints the CommonJS DSL consumer after that build.
  { ignores: ['test/types/'] },
  eslint.configs.recommended,
  tseslint.configs.recommendedTypeChecked,
  tseslint.configs.strictTypeChecked,
  tseslint.configs.stylisticTypeChecked,
  {
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      'no-fallthrough': 'off',
      '@typescript-eslint/no-non-null-assertion': 'off',
      '@typescript-eslint/no-unnecessary-condition': ['error', {
        allowConstantLoopConditions: true
      }],
      '@typescript-eslint/restrict-template-expressions': ['error', {
        allowNumber: true
      }],
    }
  },
);
