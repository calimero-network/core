import { defineConfig } from 'eslint-define-config';

export default defineConfig({
  root: true,
  env: {
    browser: true,
    es2020: true,
  },
  extends: [
    'eslint:recommended',
    'plugin:@typescript-eslint/recommended',
    'plugin:react/recommended',
    'plugin:react/jsx-runtime',
    'plugin:react-hooks/recommended',
    'react-app',
    'plugin:prettier/recommended',
  ],
  ignorePatterns: ['build', '.eslint.config.mjs'],
  parser: '@typescript-eslint/parser',
  parserOptions: {
    ecmaVersion: 'latest',
    sourceType: 'module',
    project: ['./tsconfig.json'],
  },
  settings: {
    react: {
      version: '18.2',
    },
  },
  plugins: ['@typescript-eslint', 'react-refresh'],
  rules: {
    'react/jsx-no-target-blank': 'off',
    'react/jsx-uses-react': 'error',
    'react/jsx-uses-vars': 'error',
    'react-refresh/only-export-components': [
      'warn',
      { allowConstantExport: true },
    ],
  },
});
