# PROMPT — Build Calimero Design System + Icon Library (Phased Plan)

## Role

You are a senior frontend engineer. Create a production-ready **monorepo** for **Calimero** with:

1. `@calimero/icons` — React SVG icon library
2. `@calimero/ui` — React component library using shared tokens
3. `@calimero/tokens` — design tokens compiled to CSS variables + TS
4. `apps/showcase` — Next.js app (App Router) to browse icons, components, and tokens
5. `apps/storybook` — Storybook 8 (Vite) for development, a11y, and interaction tests

Use **pnpm** + **Turborepo**, TypeScript, Vite/tsup builds, **SVGO + SVGR** for icons, and **Style Dictionary** for tokens.

### Icon Grammar (MUST ENFORCE)

- `viewBox="0 0 24 24"`
- Default **strokeWidth = 1.5**, `strokeLinecap="round"`, `strokeLinejoin="round"`
- Outline icons: `fill="none"`, **no hardcoded color** — always `stroke="currentColor"`
- Optional filled twins: `fill="currentColor"` variant
- No transforms / masks / filters

### Brand Tokens

- Accent color token: `--accent: #A5FF11` (do **not** hardcode in SVGs)
- Export tokens as CSS variables and TS

---

## Phase 0 — Bootstrap & Workspace

### Goals

- Initialize a **pnpm + Turborepo** monorepo for Calimero’s design system.
- Add Changesets for versioning & publishing.
- Create baseline configs and folder structure.

### Commands

```bash
pnpm init -y
pnpm add -D turbo typescript eslint prettier @changesets/cli @svgr/core svgo globby tsup
pnpm changeset init
```

### Root structure

```
.
├─ apps/
│  ├─ showcase/
│  └─ storybook/
├─ packages/
│  ├─ icons/
│  ├─ ui/
│  └─ tokens/
├─ tools/
│  ├─ svgo.config.json
│  └─ scripts/build-icons.mjs
├─ turbo.json
├─ package.json
└─ tsconfig.base.json
```

### Files

**`package.json` (root)**

```json
{
  "name": "calimero-design-system",
  "private": true,
  "packageManager": "pnpm@latest",
  "scripts": {
    "dev": "turbo run dev --parallel",
    "build": "turbo run build",
    "test": "turbo run test",
    "lint": "turbo run lint",
    "typecheck": "turbo run typecheck",
    "changeset": "changeset",
    "version-packages": "changeset version",
    "release": "turbo run build && changeset publish",
    "build:icons": "node tools/scripts/build-icons.mjs"
  },
  "devDependencies": {
    "turbo": "^2.0.0",
    "typescript": "^5.5.0",
    "eslint": "^9.0.0",
    "prettier": "^3.2.0",
    "@changesets/cli": "^2.27.0",
    "@svgr/core": "^8.1.0",
    "svgo": "^3.2.0",
    "globby": "^14.0.0",
    "tsup": "^8.0.0"
  }
}
```

**`turbo.json`**

```json
{
  "pipeline": {
    "build": { "dependsOn": ["^build"], "outputs": ["dist/**"] },
    "dev": { "cache": false },
    "lint": {},
    "test": {},
    "typecheck": {}
  }
}
```

**`tsconfig.base.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM"],
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "jsx": "react-jsx",
    "declaration": true,
    "declarationMap": true,
    "resolveJsonModule": true,
    "skipLibCheck": true,
    "strict": true,
    "baseUrl": ".",
    "paths": {
      "@calimero/icons": ["packages/icons/src"],
      "@calimero/ui": ["packages/ui/src"],
      "@calimero/tokens": ["packages/tokens/src"]
    }
  }
}
```

**`.gitignore` (root)**

```
node_modules
dist
.next
.storybook-out
.storybook-cache
pnpm-lock.yaml
```

### Acceptance

- `pnpm i` succeeds.
- `pnpm build` runs (no packages yet, but Turbo executes).

---

## Phase 1 — Design Tokens (`@calimero/tokens`)

### Goals

- Define brand/system tokens (CSS variables + TS).
- Ship accent color `#A5FF11` (do **not** hardcode this in SVGs).

### Create package

```bash
mkdir -p packages/tokens/src
```

**`packages/tokens/package.json`**

```json
{
  "name": "@calimero/tokens",
  "version": "0.0.0",
  "type": "module",
  "main": "./dist/index.cjs",
  "module": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "files": ["dist"],
  "scripts": {
    "build": "style-dictionary build && tsup src/index.ts --dts --format esm,cjs",
    "dev": "pnpm build",
    "lint": "eslint .",
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "devDependencies": {
    "style-dictionary": "^4.0.0",
    "tsup": "^8.0.0",
    "typescript": "^5.5.0"
  }
}
```

**`packages/tokens/style-dictionary.config.cjs`**

```js
module.exports = {
  source: ["tokens.json"],
  platforms: {
    css: {
      transformGroup: "css",
      buildPath: "dist/css/",
      files: [{ destination: "tokens.css", format: "css/variables" }],
    },
    js: {
      transformGroup: "js",
      buildPath: "dist/ts/",
      files: [{ destination: "tokens.js", format: "javascript/es6" }],
    },
  },
};
```

**`packages/tokens/tokens.json`** (seed)

```json
{
  "color": {
    "accent": { "value": "#A5FF11" },
    "fg": { "value": "#0A0A0A" },
    "fgMuted": { "value": "rgba(10,10,10,0.7)" },
    "bg": { "value": "#FFFFFF" },
    "bgMuted": { "value": "#F7F7F7" },
    "border": { "value": "#E5E5E5" }
  },
  "radius": {
    "sm": { "value": "8px" },
    "md": { "value": "12px" },
    "lg": { "value": "16px" }
  },
  "space": {
    "2": { "value": "0.5rem" },
    "3": { "value": "0.75rem" },
    "4": { "value": "1rem" },
    "6": { "value": "1.5rem" }
  },
  "font": {
    "body": { "value": "Inter, system-ui, sans-serif" }
  }
}
```

**`packages/tokens/src/index.ts`**

```ts
export * as tokens from "../dist/ts/tokens.js";
```

**Build**

```bash
pnpm -F @calimero/tokens build
```

**Acceptance**

- `dist/css/tokens.css` contains `:root { --color-accent: #A5FF11; ... }`
- JS/TS outputs exist under `dist/ts`.

---

## Phase 2 — Icon Library (`@calimero/icons`)

### Goals

- Convert raw SVGs → React components with a shared `IconBase`.
- Enforce grammar: 24×24, strokeWidth 1.5, round caps/joins, `stroke="currentColor"`.

### Create package & tooling

```bash
mkdir -p packages/icons/{raw,src} tools/scripts
```

**`tools/svgo.config.json`**

```json
{
  "multipass": true,
  "plugins": [
    { "name": "removeDimensions" },
    { "name": "removeXMLNS" },
    { "name": "convertPathData" },
    {
      "name": "removeAttrs",
      "params": { "attrs": "(id|class|data-name|fill)" }
    },
    { "name": "removeUselessStrokeAndFill" }
  ]
}
```

**`tools/scripts/build-icons.mjs`**

```js
import { readFile, writeFile, mkdir } from "fs/promises";
import path from "path";
import { fileURLToPath } from "url";
import { optimize } from "svgo";
import { transform } from "@svgr/core";
import globby from "globby";
import svgoConfig from "../svgo.config.json" assert { type: "json" };

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const RAW = path.resolve(__dirname, "../../packages/icons/raw");
const OUT = path.resolve(__dirname, "../../packages/icons/src");

await mkdir(OUT, { recursive: true });

const files = await globby("*.svg", { cwd: RAW });
const exportsList = [];

for (const filename of files) {
  const filePath = path.join(RAW, filename);
  let svg = await readFile(filePath, "utf8");

  // Normalize color & viewBox
  svg = svg
    .replace(/stroke="#[0-9A-Fa-f]{3,8}"/g, 'stroke="currentColor"')
    .replace(/fill="#[0-9A-Fa-f]{3,8}"/g, 'fill="none"');

  const { data } = optimize(svg, { path: filePath, ...svgoConfig });
  const componentName = filename
    .replace(/\.svg$/, "")
    .replace(/(^[a-z])|-(\w)/g, (_, a, b) => (a ? a.toUpperCase() : b.toUpperCase()));

  const tsx = await transform(
    data,
    {
      typescript: true,
      icon: false,
      jsxRuntime: "automatic",
      template: ({ imports, interfaces, componentName, props, jsx }, { tpl }) =>
        tpl`
${imports}
import * as React from "react";
import { IconBase, IconProps } from "./IconBase";
${interfaces}
const ${componentName} = React.forwardRef<SVGSVGElement, IconProps>((${props}) => (
  <IconBase ref={ref} {...props}>${jsx.children}</IconBase>
));
${componentName}.displayName = "${componentName}";
export { ${componentName} };
`
    },
    { componentName }
  );

  await writeFile(path.join(OUT, `${componentName}.tsx`), tsx, "utf8");
  exportsList.push(\`export { ${componentName} } from "./${componentName}";\`);
}

await writeFile(path.join(OUT, "index.ts"), \`export * from "./IconBase";\n\${exportsList.join("\\n")}\n\`);
console.log(\`Generated \${exportsList.length} icons.\`);
```

**`packages/icons/package.json`**

```json
{
  "name": "@calimero/icons",
  "version": "0.0.0",
  "type": "module",
  "exports": {
    ".": { "import": "./dist/index.js", "types": "./dist/index.d.ts" }
  },
  "files": ["dist"],
  "scripts": {
    "prebuild": "pnpm -w build:icons",
    "build": "tsup src/index.ts --dts --format esm,cjs",
    "dev": "pnpm build",
    "lint": "eslint .",
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "peerDependencies": { "react": ">=18" },
  "devDependencies": {
    "tsup": "^8.0.0",
    "typescript": "^5.5.0"
  }
}
```

**`packages/icons/src/IconBase.tsx`**

```tsx
import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  title?: string;
};

export const IconBase = React.forwardRef<SVGSVGElement, IconProps>(
  ({ size = 24, strokeWidth = 1.5, title, children, ...props }, ref) => (
    <svg
      ref={ref}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      role={title ? "img" : "presentation"}
      aria-hidden={title ? undefined : true}
      aria-label={title}
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      {...props}
    >
      {title ? <title>{title}</title> : null}
      {children}
    </svg>
  ),
);
IconBase.displayName = "IconBase";
export type { IconProps };
```

**`packages/icons/src/index.ts`**

```ts
export * from "./IconBase";
// individual icons are autogenerated into this folder and re-exported by index.ts
```

### Usage

- Place designer SVGs in `packages/icons/raw/`
- Run:

```bash
pnpm -w build:icons
pnpm -F @calimero/icons build
```

### Acceptance

- `packages/icons/src` contains generated `*.tsx` components + `index.ts`
- `dist` built with ESM + types

---

## Phase 3 — Component Library (`@calimero/ui`)

### Goals

- Minimal, accessible component primitives (e.g., Button, Card).
- Consume tokens CSS variables.

### Create package

```bash
mkdir -p packages/ui/src
```

**`packages/ui/package.json`**

```json
{
  "name": "@calimero/ui",
  "version": "0.0.0",
  "type": "module",
  "exports": {
    ".": { "import": "./dist/index.js", "types": "./dist/index.d.ts" }
  },
  "files": ["dist"],
  "scripts": {
    "build": "tsup src/index.ts --dts --format esm,cjs",
    "dev": "pnpm build",
    "lint": "eslint .",
    "typecheck": "tsc -p tsconfig.json --noEmit",
    "test": "vitest run"
  },
  "peerDependencies": { "react": ">=18" },
  "devDependencies": {
    "tsup": "^8.0.0",
    "typescript": "^5.5.0",
    "vitest": "^2.0.0",
    "@testing-library/react": "^16.0.0"
  }
}
```

**`packages/ui/src/styles.css`**

```css
@import "@calimero/tokens/dist/css/tokens.css";

:root {
  --radius: var(--radius-md);
}

.cali-btn {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.5rem 0.875rem;
  border-radius: var(--radius);
  border: 1px solid var(--color-border);
  background: var(--color-bg);
  color: var(--color-fg);
  font: 500 0.95rem/1.2 var(--font-body, Inter, system-ui, sans-serif);
  cursor: pointer;
}
.cali-btn--primary {
  background: var(--color-accent);
  color: #0a0a0a;
  border-color: var(--color-accent);
}
.cali-card {
  border: 1px solid var(--color-border);
  border-radius: var(--radius);
  background: var(--color-bg);
  padding: 1rem;
}
```

**`packages/ui/src/Button.tsx`**

```tsx
import * as React from "react";

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary";
};

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = "default", className = "", ...props }, ref) => (
    <button
      ref={ref}
      className={`cali-btn ${variant === "primary" ? "cali-btn--primary" : ""} ${className}`}
      {...props}
    />
  ),
);
Button.displayName = "Button";
```

**`packages/ui/src/Card.tsx`**

```tsx
import * as React from "react";

export const Card: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  className = "",
  ...props
}) => <div className={`cali-card ${className}`} {...props} />;
```

**`packages/ui/src/index.ts`**

```ts
export * from "./Button";
export * from "./Card";
export { default as styles } from "./styles.css";
```

**Build**

```bash
pnpm -F @calimero/ui build
```

**Acceptance**

- `dist` contains ESM + types; CSS can be imported by apps/Storybook.

---

## Phase 4 — Storybook App (`apps/storybook`)

### Goals

- Developer playground with Controls & A11y.
- Render UI components and a sample icon story.

### Create app

```bash
mkdir -p apps/storybook/.storybook
```

**`apps/storybook/package.json`**

```json
{
  "name": "storybook",
  "private": true,
  "scripts": {
    "dev": "storybook dev -p 6006",
    "build": "storybook build -o ./.out"
  },
  "devDependencies": {
    "@storybook/react-vite": "^8.1.0",
    "@storybook/addon-essentials": "^8.1.0",
    "@storybook/addon-a11y": "^8.1.0",
    "@storybook/addon-interactions": "^8.1.0",
    "react": "^18.2.0",
    "react-dom": "^18.2.0"
  }
}
```

**`apps/storybook/.storybook/main.ts`**

```ts
import type { StorybookConfig } from "@storybook/react-vite";
const config: StorybookConfig = {
  stories: ["../../packages/**/*.stories.@(tsx|mdx)"],
  addons: [
    "@storybook/addon-essentials",
    "@storybook/addon-a11y",
    "@storybook/addon-interactions",
  ],
  framework: { name: "@storybook/react-vite", options: {} },
};
export default config;
```

**`apps/storybook/.storybook/preview.tsx`**

```tsx
import "@calimero/tokens/dist/css/tokens.css";
import "@calimero/ui/dist/styles.css";
export const parameters = { controls: { expanded: true } };
```

### Example stories (inside packages)

**`packages/ui/src/Button.stories.tsx`**

```tsx
import type { Meta, StoryObj } from "@storybook/react";
import { Button } from "./Button";

const meta: Meta<typeof Button> = {
  title: "UI/Button",
  component: Button,
  args: { children: "Click me" },
};
export default meta;

type S = StoryObj<typeof Button>;
export const Default: S = {};
export const Primary: S = { args: { variant: "primary" } };
```

**`packages/icons/src/Icon.stories.tsx`**

```tsx
import type { Meta, StoryObj } from "@storybook/react";
import { IconBase } from "./IconBase";

const meta: Meta<typeof IconBase> = {
  title: "Icons/IconBase",
  component: IconBase,
  args: {
    children: <circle cx="12" cy="12" r="9" />,
    strokeWidth: 1.5,
    size: 48,
  },
};
export default meta;

export const Playground: StoryObj<typeof IconBase> = {};
```

### Run

```bash
pnpm -F storybook dev
```

### Acceptance

- Storybook loads with Button stories and IconBase playground.

---

## Phase 5 — Showcase App (`apps/showcase`)

### Goals

- Public-facing Next.js app to browse **icons, components, and tokens**.
- Icons grid with search + click-to-copy import line.

### Create app

```bash
mkdir -p apps/showcase/app/{icons,tokens,components}
```

**`apps/showcase/package.json`**

```json
{
  "name": "showcase",
  "private": true,
  "scripts": {
    "dev": "next dev -p 3000",
    "build": "next build",
    "start": "next start -p 3000"
  },
  "dependencies": {
    "next": "^14.2.0",
    "react": "^18.2.0",
    "react-dom": "^18.2.0",
    "@calimero/icons": "0.0.0",
    "@calimero/ui": "0.0.0",
    "@calimero/tokens": "0.0.0"
  }
}
```

**`apps/showcase/app/layout.tsx`**

```tsx
import "@calimero/tokens/dist/css/tokens.css";
import "@calimero/ui/dist/styles.css";
import "./globals.css";
import type { ReactNode } from "react";

export const metadata = {
  title: "Calimero Design System",
  description: "Components, Icons, Tokens",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body style={{ margin: 0, fontFamily: "Inter, system-ui, sans-serif" }}>
        <div
          style={{
            padding: "1rem",
            borderBottom: "1px solid var(--color-border)",
          }}
        >
          <strong>Calimero</strong> — Design System
        </div>
        <main style={{ padding: "1rem" }}>{children}</main>
      </body>
    </html>
  );
}
```

**`apps/showcase/app/page.tsx`**

```tsx
export default function Page() {
  return (
    <div>
      <h1>Welcome</h1>
      <ul>
        <li>
          <a href="/icons">Icons</a>
        </li>
        <li>
          <a href="/components">Components</a>
        </li>
        <li>
          <a href="/tokens">Tokens</a>
        </li>
      </ul>
    </div>
  );
}
```

**`apps/showcase/app/icons/page.tsx`**

```tsx
"use client";
import * as React from "react";
import * as Icons from "@calimero/icons";

export default function IconsPage() {
  const [q, setQ] = React.useState("");
  const entries = Object.entries(Icons).filter(([name]) =>
    name.toLowerCase().includes(q.toLowerCase())
  );
  const copy = (name: string) =>
    navigator.clipboard.writeText(\`import { \${name} } from "@calimero/icons"\`);

  return (
    <div style={{ display: "grid", gap: "1rem" }}>
      <h1>Icons</h1>
      <input
        placeholder="Search icons..."
        value={q}
        onChange={e => setQ(e.target.value)}
        style={{ padding: ".5rem .75rem", border: "1px solid var(--color-border)", borderRadius: "8px" }}
      />
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))",
          gap: "1rem"
        }}
      >
        {entries.map(([name, Icon]) => (
          <button
            key={name}
            onClick={() => copy(name)}
            style={{
              border: "1px solid var(--color-border)",
              borderRadius: "12px",
              padding: "1rem",
              textAlign: "left",
              background: "var(--color-bg)",
              cursor: "pointer"
            }}
            title="Click to copy import"
          >
            <div style={{ display: "flex", justifyContent: "center", marginBottom: ".5rem" }}>
              {/* @ts-ignore */}
              <Icon size={32} strokeWidth={1.5} aria-hidden />
            </div>
            <div style={{ fontSize: "12px", opacity: 0.8 }}>{name}</div>
            <div style={{ fontSize: "10px", opacity: 0.6 }}>Click to copy import</div>
          </button>
        ))}
      </div>
    </div>
  );
}
```

**`apps/showcase/app/tokens/page.tsx`**

```tsx
export default function Tokens() {
  const rows = [
    ["--color-accent", "Accent brand (#A5FF11)"],
    ["--color-fg", "Foreground"],
    ["--color-bg", "Background"],
    ["--color-border", "Borders"]
  ];
  return (
    <div style={{ display: "grid", gap: "1rem" }}>
      <h1>Tokens</h1>
      <div style={{ display: "grid", gap: ".75rem" }}>
        {rows.map(([name, label]) => (
          <div key={name} style={{ display: "flex", alignItems: "center", gap: "1rem" }}>
            <div style={{ width: 32, height: 32, borderRadius: 8, background: ` + "`" + `var(${name})` + "`" + `, border: "1px solid var(--color-border)" }} />
            <code>{name}</code>
            <span style={{ opacity: 0.7 }}>{label}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
```

**`apps/showcase/app/components/page.tsx`**

```tsx
"use client";
import { Button, Card } from "@calimero/ui";

export default function Components() {
  return (
    <div style={{ display: "grid", gap: "1rem" }}>
      <h1>Components</h1>
      <div style={{ display: "flex", gap: ".75rem" }}>
        <Button>Default</Button>
        <Button variant="primary">Primary</Button>
      </div>
      <Card>Card content</Card>
    </div>
  );
}
```

**`apps/showcase/app/globals.css`**

```css
/* You can customize page-level styles here */
```

### Run

```bash
pnpm -F showcase dev
```

### Acceptance

- Icons page shows generated icons, searchable, copy-to-clipboard works.
- Tokens & Components pages render correctly.

---

## Phase 6 — CI, Versioning & Release

### Goals

- Version & publish packages with Changesets.
- Optional CI for build/test and visual checks.

### Changesets workflow

```bash
pnpm changeset            # create a changeset (choose @calimero/* packages)
pnpm version-packages     # bump versions & update changelogs
pnpm release              # build & publish to registry (ensure auth set up)
```

### Suggested GitHub Actions (outline)

**`.github/workflows/ci.yml`**

```yaml
name: CI
on:
  pull_request:
  push:
jobs:
  build-test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v3
        with: { version: 9 }
      - uses: actions/setup-node@v4
        with: { node-version: 20, cache: "pnpm" }
      - run: pnpm i
      - run: pnpm build
      - run: pnpm test
```

**`.github/workflows/release.yml`** (manual or on tag)

```yaml
name: Release
on:
  workflow_dispatch:
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v3
        with: { version: 9 }
      - uses: actions/setup-node@v4
        with:
          {
            node-version: 20,
            cache: "pnpm",
            registry-url: "https://registry.npmjs.org",
          }
      - run: pnpm i
      - run: pnpm build
      - run: pnpm changeset publish
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

### Visual regression (optional)

- **Storybook Chromatic** or **Playwright** screenshot tests on the Showcase.
- Add a Showcase “icon wall” route that renders all icons at 16/20/24 px in light/dark to spot wobble.

### Definition of Done

- `pnpm dev` runs Storybook + Showcase concurrently.
- `@calimero/tokens`, `@calimero/icons`, `@calimero/ui` build & typecheck.
- Icons generated from raw SVGs with `strokeWidth=1.5`, round caps/joins, `currentColor`.
- Showcase pages: **Icons**, **Components**, **Tokens** operational.
- Changesets publish flow tested on a dry run (or to a private scope).
