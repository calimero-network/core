# @calimero-network/mero-ui

## 1.5.1

### Patch Changes

- 09f7c52: RichTextEditor: render the `placeholder` prop. The Tiptap `Placeholder`
  extension was never registered, so the prop was accepted and silently showed
  nothing. (Published to npm on 2026-08-21; recorded here after the fact,
  because the release job versions on the runner and never pushes that commit
  back.)

## 1.5.0

### Minor Changes

- 05b74d5: ### Enhanced Components

  #### Button Component
  - **Polymorphic Support**: Added `as` prop to render Button as any element (Link, anchor, etc.)
  - **Extended API**: Added `size` (sm, md, lg, xl), `leftIcon`, `rightIcon`, `fullWidth`, and `rounded` props
  - **Additional Variants**: Added `outline`, `ghost`, and `destructive` variants
  - **Breaking Change**: Removed `loading` prop; use composition pattern with `Spinner` component and `disabled` state instead
  - **Fix**: Enabled hover/active states for polymorphic Link rendering

  #### Card Component
  - **Color Control**: Added `color` prop to `CardTitle` and `CardContent` for brand color customization
  - **Design Tokens**: Updated default text colors to use design tokens (`neutral[200]` and `neutral[300]`)

  #### Spinner Component
  - **Fix**: Fixed rotation center by moving animation to group element with proper `transform-origin`

  ### Removed
  - Removed `DataList` component (unused simple layout component)

## 1.4.0

### Minor Changes

- a15245b: Replace text color on paste
- b43d043: Replace paste text color

## 1.3.0

### Minor Changes

- 5d8b26d: Remove link from rich text editor toolbar

## 1.2.0

### Minor Changes

- 7c9371b: Reduce default rich text editor toolbar

## 1.1.0

### Minor Changes

- 57b9bee: CSS fix for rich text editor

## 1.0.0

### Major Changes

- a2b21d9: patch: add dynamic input for rich text editor

### Minor Changes

- f24c0f4: Dynamic input to rich text editor

### Patch Changes

- Updated dependencies [f24c0f4]
  - @calimero-network/mero-icons@0.0.6

## 0.3.7

### Patch Changes

- Publish latest changes.
- bf330c8: Fix workspace dependency references for proper external consumption
  - Resolve workspace:\* references to actual version numbers during publishing
  - Ensure external consumers can install packages without workspace reference errors
  - Maintain local development workflow with workspace references

- Updated dependencies
  - @calimero-network/mero-icons@0.0.5

## 0.3.6

### Patch Changes

- Fix workspace dependency references for proper external consumption
  - Resolve workspace:\* references to actual version numbers during publishing
  - Ensure external consumers can install packages without workspace reference errors
  - Maintain local development workflow with workspace references

## 0.3.3

### Patch Changes

- 5a1d22c: chore: fix lockfile synchronization after version bump
  - Update pnpm-lock.yaml to match package.json versions
  - Resolve ERR_PNPM_OUTDATED_LOCKFILE error in CI environments
  - Ensure consistent dependency resolution across all packages

- feat: Upgrade all packages to latest versions
  - Upgrade Storybook from 8.6.14 to 9.1.10
  - Upgrade React from 18.3.1 to 19.1.0
  - Upgrade React types to 19.2.0
  - Upgrade Vite to 7.1.8
  - Remove @storybook/addon-essentials (now built into Storybook 9.x)
  - Update all Storybook addons to 9.1.10
  - Fix dynamic require issues with React 19
  - Synchronize lockfile across all environments

- Updated dependencies
  - @calimero-network/mero-icons@0.0.4

## 0.3.2

### Patch Changes

- fix: resolve dynamic require of React issue in UI package
  - Add esbuild banner to provide require function for React modules
  - Fix use-sync-external-store dynamic require issue in ESM builds
  - Update Storybook Vite config to handle module resolution
  - Downgrade all React packages to version 18 for compatibility
  - Fix TypeScript types in Card component for tooltipIcon prop

## 0.3.1

### Patch Changes

- fix: resolve RichTextEditor build and deployment issues
  - Bundle Tiptap packages in UI build to fix Storybook module resolution
  - Add noExternal configuration to tsup.config.ts for Tiptap dependencies
  - Fix Vercel deployment issues with Tiptap module resolution
  - Re-enable RichTextEditor component exports and Storybook stories
  - Storybook build now works successfully with RichTextEditor component
  - Bundle size increased to ~1.3MB due to Tiptap bundling for better compatibility

## 0.3.0

### Minor Changes

- feat: add RichTextEditor component with Tiptap integration
  - Add new RichTextEditor component with full rich text editing capabilities
  - Integrate Tiptap editor with StarterKit and extensions (Color, TextAlign, Underline, Link)
  - Support for bold, italic, underline, headings, lists, quotes, code blocks
  - Clean default text-based toolbar icons (B, I, U, H, •, ", </>, 🔗, A, H)
  - Full TypeScript support with proper prop types
  - Design system integration with tokens and styling
  - Storybook stories demonstrating all features
  - Controlled and uncontrolled component modes
  - Accessibility features with proper ARIA labels

## 0.2.0

### Minor Changes

- # Major UI Package Update

  ## New Components Added

  ### Grid System
  - **Grid**: Flexible CSS Grid container with configurable columns, gap, and alignment
  - **GridItem**: Grid items with column/row spanning and positioning support

  ### Menu System
  - **Menu**: Dark-themed menu container with multiple variants and sizes
  - **MenuItem**: Interactive menu items with icons, states, and click handlers
  - **MenuGroup**: Grouped menu items with optional labels
  - **MenuDivider**: Visual separators between menu groups

  ### Navbar System
  - **Navbar**: Responsive navigation bar with fixed positioning support
  - **NavbarBrand**: Brand/logo section with click handlers
  - **NavbarItem**: Navigation items with active states and icons
  - **NavbarMenu**: Flexible menu containers with alignment options
  - **NavbarToggle**: Mobile hamburger menu toggle

  ## Enhancements
  - **Card Component**: Added `variant` prop for rectangle/rounded styles
  - **Comprehensive Storybook Stories**: Interactive examples for all new components
  - **TypeScript Support**: Full type safety for all components
  - **Dark Theme**: Consistent styling across all components

  ## Breaking Changes

  None - all changes are additive and backward compatible.

  ## Migration Guide

  No migration required. All existing components continue to work as before.

## 0.1.0

### Minor Changes

- Release: add secondary and semantic button variants; fix Storybook stability and icon packing; externalize React in builds; align peer deps; dark theme consistency across components.

### Patch Changes

- Updated dependencies
  - @calimero-network/mero-icons@0.0.3

## 0.0.2

### Patch Changes

- enable TypeScript types generation for all packages
- Updated dependencies
  - @calimero-network/mero-icons@0.0.2

## 0.0.1

### Patch Changes

- b346837: Initial version
- Updated dependencies [b346837]
  - @calimero-network/mero-icons@0.0.1
