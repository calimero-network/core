import type { Meta, StoryObj } from "@storybook/react-vite";
import { CommandPalette } from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";

const meta: Meta<typeof CommandPalette> = {
  title: "Navigation/Command Palette",
  component: CommandPalette,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    items: {
      control: { type: "object" },
      description: "Array of command items",
    },
    placeholder: {
      control: { type: "text" },
      description: "Search placeholder text",
    },
    emptyMessage: {
      control: { type: "text" },
      description: "Message when no results found",
    },
    maxHeight: {
      control: { type: "number" },
      description: "Maximum height of results",
    },
    hotkey: {
      control: { type: "text" },
      description: "Keyboard shortcut to open",
    },
    showCategories: {
      control: { type: "boolean" },
      description: "Show command categories",
    },
  },
};

export default meta;
type Story = StoryObj<typeof CommandPalette>;

const sampleCommands = [
  {
    id: "new-file",
    title: "New File",
    description: "Create a new file",
    icon: <Icon name="file-plus" size={16} />,
    keywords: ["create", "new", "file"],
    category: "File",
    action: () => alert("New file created!"),
  },
  {
    id: "open-file",
    title: "Open File",
    description: "Open an existing file",
    icon: <Icon name="folder-open" size={16} />,
    keywords: ["open", "file", "browse"],
    category: "File",
    action: () => alert("File opened!"),
  },
  {
    id: "save-file",
    title: "Save File",
    description: "Save the current file",
    icon: <Icon name="save" size={16} />,
    keywords: ["save", "file", "store"],
    category: "File",
    action: () => alert("File saved!"),
  },
  {
    id: "cut",
    title: "Cut",
    description: "Cut selected text",
    icon: <Icon name="scissors" size={16} />,
    keywords: ["cut", "copy", "text"],
    category: "Edit",
    action: () => alert("Text cut!"),
  },
  {
    id: "copy",
    title: "Copy",
    description: "Copy selected text",
    icon: <Icon name="copy" size={16} />,
    keywords: ["copy", "duplicate", "text"],
    category: "Edit",
    action: () => alert("Text copied!"),
  },
  {
    id: "paste",
    title: "Paste",
    description: "Paste from clipboard",
    icon: <Icon name="clipboard" size={16} />,
    keywords: ["paste", "clipboard", "text"],
    category: "Edit",
    action: () => alert("Text pasted!"),
  },
  {
    id: "undo",
    title: "Undo",
    description: "Undo last action",
    icon: <Icon name="undo" size={16} />,
    keywords: ["undo", "reverse", "back"],
    category: "Edit",
    action: () => alert("Action undone!"),
  },
  {
    id: "redo",
    title: "Redo",
    description: "Redo last undone action",
    icon: <Icon name="redo" size={16} />,
    keywords: ["redo", "repeat", "forward"],
    category: "Edit",
    action: () => alert("Action redone!"),
  },
  {
    id: "search",
    title: "Search",
    description: "Search in current file",
    icon: <Icon name="search" size={16} />,
    keywords: ["search", "find", "look"],
    category: "View",
    action: () => alert("Search opened!"),
  },
  {
    id: "replace",
    title: "Replace",
    description: "Find and replace text",
    icon: <Icon name="replace" size={16} />,
    keywords: ["replace", "find", "substitute"],
    category: "View",
    action: () => alert("Replace opened!"),
  },
  {
    id: "zoom-in",
    title: "Zoom In",
    description: "Increase zoom level",
    icon: <Icon name="zoom-in" size={16} />,
    keywords: ["zoom", "in", "magnify"],
    category: "View",
    action: () => alert("Zoomed in!"),
  },
  {
    id: "zoom-out",
    title: "Zoom Out",
    description: "Decrease zoom level",
    icon: <Icon name="zoom-out" size={16} />,
    keywords: ["zoom", "out", "reduce"],
    category: "View",
    action: () => alert("Zoomed out!"),
  },
  {
    id: "settings",
    title: "Settings",
    description: "Open application settings",
    icon: <Icon name="settings" size={16} />,
    keywords: ["settings", "preferences", "config"],
    category: "Tools",
    action: () => alert("Settings opened!"),
  },
  {
    id: "help",
    title: "Help",
    description: "Open help documentation",
    icon: <Icon name="help-circle" size={16} />,
    keywords: ["help", "documentation", "guide"],
    category: "Tools",
    action: () => alert("Help opened!"),
  },
  {
    id: "about",
    title: "About",
    description: "Show application information",
    icon: <Icon name="info" size={16} />,
    keywords: ["about", "info", "version"],
    category: "Tools",
    action: () => alert("About dialog opened!"),
  },
];

export const Default: Story = {
  args: {
    items: sampleCommands,
  },
};

export const WithCustomPlaceholder: Story = {
  args: {
    items: sampleCommands,
    placeholder: "Type a command or search...",
  },
};

export const WithCustomEmptyMessage: Story = {
  args: {
    items: sampleCommands,
    emptyMessage: "No commands found. Try a different search term.",
  },
};

export const WithoutCategories: Story = {
  args: {
    items: sampleCommands,
    showCategories: false,
  },
};

export const WithCustomHotkey: Story = {
  args: {
    items: sampleCommands,
    hotkey: "ctrl+space",
  },
};

export const WithMaxHeight: Story = {
  args: {
    items: sampleCommands,
    maxHeight: 300,
  },
};

export const WithDisabledItems: Story = {
  args: {
    items: [
      ...sampleCommands.slice(0, 3),
      {
        ...sampleCommands[3],
        disabled: true,
      },
      ...sampleCommands.slice(4),
    ],
  },
};

export const NavigationCommands: Story = {
  render: () => {
    const navigationCommands = [
      {
        id: "home",
        title: "Go to Home",
        description: "Navigate to the home page",
        icon: <Icon name="home" size={16} />,
        keywords: ["home", "dashboard", "main"],
        category: "Navigation",
        action: () => alert("Navigated to home!"),
      },
      {
        id: "profile",
        title: "Go to Profile",
        description: "Navigate to your profile page",
        icon: <Icon name="user" size={16} />,
        keywords: ["profile", "account", "user"],
        category: "Navigation",
        action: () => alert("Navigated to profile!"),
      },
      {
        id: "settings",
        title: "Go to Settings",
        description: "Navigate to settings page",
        icon: <Icon name="settings" size={16} />,
        keywords: ["settings", "preferences", "config"],
        category: "Navigation",
        action: () => alert("Navigated to settings!"),
      },
      {
        id: "notifications",
        title: "Go to Notifications",
        description: "Navigate to notifications page",
        icon: <Icon name="bell" size={16} />,
        keywords: ["notifications", "alerts", "messages"],
        category: "Navigation",
        action: () => alert("Navigated to notifications!"),
      },
      {
        id: "back",
        title: "Go Back",
        description: "Navigate to previous page",
        icon: <Icon name="arrow-left" size={16} />,
        keywords: ["back", "previous", "return"],
        category: "Navigation",
        action: () => alert("Navigated back!"),
      },
      {
        id: "forward",
        title: "Go Forward",
        description: "Navigate to next page",
        icon: <Icon name="arrow-right" size={16} />,
        keywords: ["forward", "next", "advance"],
        category: "Navigation",
        action: () => alert("Navigated forward!"),
      },
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Navigation Command Palette
        </h3>
        <p style={{ color: "#FFFFFF", marginBottom: "20px" }}>
          Press{" "}
          <kbd
            style={{
              background: "var(--color-background-secondary)",
              padding: "2px 6px",
              borderRadius: "4px",
              fontSize: "12px",
            }}
          >
            ⌘K
          </kbd>{" "}
          to open the command palette
        </p>
        <CommandPalette items={navigationCommands} />
      </div>
    );
  },
};

export const QuickActions: Story = {
  render: () => {
    const quickActions = [
      {
        id: "create-project",
        title: "Create New Project",
        description: "Start a new project from scratch",
        icon: <Icon name="plus-circle" size={16} />,
        keywords: ["create", "new", "project", "start"],
        category: "Projects",
        action: () => alert("New project created!"),
      },
      {
        id: "import-project",
        title: "Import Project",
        description: "Import an existing project",
        icon: <Icon name="upload" size={16} />,
        keywords: ["import", "upload", "project", "existing"],
        category: "Projects",
        action: () => alert("Project import started!"),
      },
      {
        id: "export-project",
        title: "Export Project",
        description: "Export current project",
        icon: <Icon name="download" size={16} />,
        keywords: ["export", "download", "project", "save"],
        category: "Projects",
        action: () => alert("Project exported!"),
      },
      {
        id: "share-project",
        title: "Share Project",
        description: "Share project with others",
        icon: <Icon name="share" size={16} />,
        keywords: ["share", "collaborate", "project", "team"],
        category: "Projects",
        action: () => alert("Project shared!"),
      },
      {
        id: "duplicate-project",
        title: "Duplicate Project",
        description: "Create a copy of current project",
        icon: <Icon name="copy" size={16} />,
        keywords: ["duplicate", "copy", "project", "clone"],
        category: "Projects",
        action: () => alert("Project duplicated!"),
      },
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Quick Actions
        </h3>
        <CommandPalette
          items={quickActions}
          placeholder="Search for actions..."
          hotkey="ctrl+shift+p"
        />
      </div>
    );
  },
};

export const SearchExample: Story = {
  render: () => {
    const searchCommands = [
      {
        id: "search-files",
        title: "Search Files",
        description: "Search for files in the project",
        icon: <Icon name="search" size={16} />,
        keywords: ["search", "files", "find", "look"],
        category: "Search",
        action: () => alert("File search opened!"),
      },
      {
        id: "search-code",
        title: "Search Code",
        description: "Search for code patterns",
        icon: <Icon name="code" size={16} />,
        keywords: ["search", "code", "pattern", "function"],
        category: "Search",
        action: () => alert("Code search opened!"),
      },
      {
        id: "search-symbols",
        title: "Search Symbols",
        description: "Search for symbols and definitions",
        icon: <Icon name="hash" size={16} />,
        keywords: ["search", "symbols", "definitions", "variables"],
        category: "Search",
        action: () => alert("Symbol search opened!"),
      },
      {
        id: "search-references",
        title: "Search References",
        description: "Find all references to a symbol",
        icon: <Icon name="link" size={16} />,
        keywords: ["search", "references", "usages", "calls"],
        category: "Search",
        action: () => alert("Reference search opened!"),
      },
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Search Commands
        </h3>
        <CommandPalette
          items={searchCommands}
          placeholder="Search for anything..."
          emptyMessage="No search results found. Try a different term."
        />
      </div>
    );
  },
};
