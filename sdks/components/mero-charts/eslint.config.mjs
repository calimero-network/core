import tseslint from '@typescript-eslint/eslint-plugin';
import tsparser from '@typescript-eslint/parser';

export default [
  {
    files: ['src/**/*.ts', 'src/**/*.tsx', 'test/**/*.ts', 'test/**/*.tsx'],
    languageOptions: {
      parser: tsparser,
      parserOptions: {
        ecmaVersion: 2022,
        sourceType: 'module',
        ecmaFeatures: { jsx: true },
      },
    },
    plugins: {
      '@typescript-eslint': tseslint,
    },
    rules: {
      ...tseslint.configs.recommended.rules,
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-unused-vars': ['warn', { argsIgnorePattern: '^_' }],
    },
  },
  {
    // TimeSeries.tsx opts out of type checking (`// @ts-nocheck`) around
    // recharts' typings; it was written that way before the move to core.
    files: ['src/TimeSeries.tsx'],
    rules: {
      '@typescript-eslint/ban-ts-comment': 'off',
    },
  },
  {
    ignores: ['dist/', 'node_modules/', 'examples/', 'tools/'],
  },
];
