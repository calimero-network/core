# @calimero-network/mero-ui

A comprehensive React UI component library built for the Calimero design system. This package provides a complete set of accessible, customizable components with built-in dark theme support.

## Installation

```bash
npm install @calimero-network/mero-ui
# or
yarn add @calimero-network/mero-ui
# or
pnpm add @calimero-network/mero-ui
```

## Quick Start

### 1. Import CSS Variables

**Important**: You must import the CSS variables for proper styling. Choose one of the following methods:

#### Option A: Import CSS file directly

```css
/* In your main CSS file */
@import "@calimero-network/mero-ui/styles.css";
```

#### Option B: Import in HTML

```html
<link
  rel="stylesheet"
  href="node_modules/@calimero-network/mero-ui/dist/styles.css"
/>
```

#### Option C: Programmatic injection

```javascript
import { cssVariables } from "@calimero-network/mero-ui";

// Inject CSS variables into your document
const style = document.createElement("style");
style.textContent = cssVariables;
document.head.appendChild(style);
```

### 2. Import and use components

```jsx
import React from "react";
import { Button, Card, Text, Input } from "@calimero-network/mero-ui";

function App() {
  return (
    <div>
      <Card>
        <Text size="lg" weight="bold">
          Welcome to Calimero UI
        </Text>
        <Input placeholder="Enter your name" />
        <Button variant="primary">Get Started</Button>
      </Card>
    </div>
  );
}

export default App;
```

## Available Components

### Layout & Structure

- **Box** - Flexible container component
- **Flex** - Flexbox container
- **Stack** - Vertical/horizontal stack layout
- **Grid** & **GridItem** - CSS Grid layout system
- **Center** - Center content horizontally and vertically
- **Spacer** - Add spacing between elements
- **Layout** - Main layout wrapper
- **Container** - Responsive container
- **AuthLayout** - Authentication page layout

### Navigation

- **Navbar** - Top navigation bar
- **Sidebar** - Side navigation panel
- **Breadcrumbs** - Navigation breadcrumbs
- **Menu** - Dropdown and context menus
- **Tabs** - Tab navigation
- **Accordion** - Collapsible content sections
- **CommandPalette** - Command search interface

### Forms & Input

- **Input** - Text input field
- **Textarea** - Multi-line text input
- **NumberInput** - Numeric input with controls
- **Select** - Dropdown selection
- **Checkbox** - Checkbox input
- **Radio** & **RadioGroup** - Radio button inputs
- **Switch** - Toggle switch
- **DatePicker** - Date selection
- **TimePicker** - Time selection
- **FileUpload** - File upload component
- **SearchInput** - Search input with suggestions
- **TagInput** - Tag input with autocomplete
- **RichTextEditor** - Rich text editing with Tiptap
- **Form** - Form wrapper with validation
- **BuildWizard** - Multi-step form wizard

### Data Display

- **Table** - Data tables with sorting and filtering
- **DataTable** - Advanced data table
- **EditableTable** - Editable data table
- **TreeTable** - Hierarchical data table
- **List** - List display component
- **Tree** - Tree view component
- **Calendar** - Calendar component
- **Timeline** - Timeline display
- **StatCard** - Statistics display card
- **MetricCard** - Metrics with charts
- **TrendIndicator** - Trend visualization
- **EmptyState** - Empty state displays
- **Skeleton** - Loading placeholders

### Feedback & Status

- **Alert** - Alert messages
- **Banner** - Banner notifications
- **Toast** - Toast notifications
- **NotificationCenter** - Notification management
- **Badge** - Status badges
- **Progress** - Progress indicators
- **Loader** - Loading spinners
- **Spinner** - Loading spinner
- **Modal** - Modal dialogs
- **Popover** - Popover content
- **Tooltip** - Tooltip overlays

### Media & Content

- **Avatar** - User avatar display
- **Image** - Image component
- **Icon** - Icon component
- **Divider** - Visual separators
- **FilePreview** - File preview component

### Typography

- **Text** - Text component with variants
- **Heading** - Heading components
- **Code** - Code display
- **Link** - Link component

### Utility

- **Tag** - Tag component
- **CopyToClipboard** - Copy to clipboard utility
- **SearchHighlight** - Search result highlighting

## Design Tokens

The package includes comprehensive design tokens for consistent styling:

### Colors

```javascript
import { colors } from "@calimero-network/mero-ui";

// Brand colors
colors.brand[600]; // Primary brand color
colors.brand[700]; // Darker brand color

// Neutral colors
colors.neutral[200]; // Light gray
colors.neutral[800]; // Dark gray

// Semantic colors
colors.semantic.success; // Success green
colors.semantic.error; // Error red
colors.semantic.warning; // Warning orange
colors.semantic.info; // Info blue

// Background colors
colors.background.primary; // Primary background
colors.background.secondary; // Secondary background
```

### Spacing

```javascript
import { spacing } from "@calimero-network/mero-ui";

spacing[2]; // 0.5rem
spacing[4]; // 1rem
spacing[6]; // 1.5rem
```

### Typography

```javascript
import { fonts } from "@calimero-network/mero-ui";

fonts.body; // Main body font
fonts.heading; // Heading font
fonts.secondary; // Secondary font
```

### Border Radius

```javascript
import { radius } from "@calimero-network/mero-ui";

radius.sm; // 8px
radius.md; // 12px
radius.lg; // 16px
```

## CSS Variables

All design tokens are available as CSS variables:

```css
.my-component {
  background-color: var(--color-background-primary);
  color: var(--color-neutral-200);
  font-family: var(--font-body);
  border-radius: var(--radius-md);
  padding: var(--space-4);
  border: 1px solid var(--color-neutral-600);
}
```

### Available CSS Variables

#### Colors

- `--color-neutral-200` to `--color-neutral-900`
- `--color-brand-100`, `--color-brand-600` to `--color-brand-900`
- `--color-background-primary`, `--color-background-secondary`, `--color-background-tertiary`
- `--color-semantic-success`, `--color-semantic-warning`, `--color-semantic-error`, `--color-semantic-info`

#### Spacing

- `--space-2`, `--space-3`, `--space-4`, `--space-6`

#### Border Radius

- `--radius-sm`, `--radius-md`, `--radius-lg`

#### Fonts

- `--font-body`, `--font-heading`, `--font-secondary`

## Theming

The design system is built with a dark theme as the default. All components automatically use the appropriate colors and styling for the dark theme. The CSS variables ensure consistent theming across all components.

## TypeScript Support

Full TypeScript support is included with comprehensive type definitions for all components and their props.

```typescript
import { Button, ButtonProps } from '@calimero-network/mero-ui';

const MyButton: React.FC<ButtonProps> = ({ variant, children, ...props }) => {
  return <Button variant={variant} {...props}>{children}</Button>;
};
```

## Accessibility

All components are built with accessibility in mind, including:

- Proper ARIA attributes
- Keyboard navigation support
- Screen reader compatibility
- Focus management
- Color contrast compliance

## Browser Support

- Chrome (latest)
- Firefox (latest)
- Safari (latest)
- Edge (latest)

## Contributing

This package is part of the Calimero design system. For contributing guidelines and development setup, please refer to the main design system repository.

## License

This package is part of the Calimero project. Please refer to the main project for licensing information.

## Changelog

See [CHANGELOG.md](./CHANGELOG.md) for a detailed list of changes and updates.
