module.exports = {
  root: true,
  parser: '@typescript-eslint/parser',
  plugins: ['@typescript-eslint'],
  extends: ['eslint:recommended', 'plugin:@typescript-eslint/recommended'],
  env: {
    browser: true,
    node: true,
    es2020: true,
  },
  ignorePatterns: [
    // `**/` because `docs/` builds into its own `dist/`: a bare `dist/**` only
    // matches the package root, so anyone who ran the docs build locally then
    // linted got 100+ errors out of Starlight's minified output.
    '**/dist/**',
    '**/.astro/**',
    'lib/**',
    '**/*.test.ts',
    '**/*.spec.ts',
    'tests/**',
  ],
  rules: {
    '@typescript-eslint/no-non-null-assertion': 'error',
    '@typescript-eslint/no-unused-vars': [
      'warn',
      {
        argsIgnorePattern: '^_',
        varsIgnorePattern: '^_',
      },
    ],
  },
};
