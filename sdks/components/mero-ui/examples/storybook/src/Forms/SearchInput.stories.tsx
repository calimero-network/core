import type { Meta, StoryObj } from "@storybook/react-vite";
import { SearchInput } from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof SearchInput> = {
  title: "Forms/Search Input",
  component: SearchInput,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    value: {
      control: { type: "text" },
      description: "Input value",
    },
    placeholder: {
      control: { type: "text" },
      description: "Input placeholder",
    },
    disabled: {
      control: { type: "boolean" },
      description: "Disable the input",
    },
    loading: {
      control: { type: "boolean" },
      description: "Show loading state",
    },
    clearable: {
      control: { type: "boolean" },
      description: "Show clear button",
    },
    showSuggestions: {
      control: { type: "boolean" },
      description: "Show suggestions dropdown",
    },
    maxSuggestions: {
      control: { type: "number" },
      description: "Maximum number of suggestions",
    },
    minLength: {
      control: { type: "number" },
      description: "Minimum length to show suggestions",
    },
    debounceMs: {
      control: { type: "number" },
      description: "Debounce delay in milliseconds",
    },
    size: {
      control: { type: "select" },
      options: ["small", "medium", "large"],
      description: "Input size",
    },
    error: {
      control: { type: "boolean" },
      description: "Show error state",
    },
    errorMessage: {
      control: { type: "text" },
      description: "Error message",
    },
    label: {
      control: { type: "text" },
      description: "Input label",
    },
    required: {
      control: { type: "boolean" },
      description: "Required field",
    },
    showCategories: {
      control: { type: "boolean" },
      description: "Show suggestion categories",
    },
    emptyMessage: {
      control: { type: "text" },
      description: "Empty state message",
    },
    hotkey: {
      control: { type: "text" },
      description: "Keyboard shortcut to focus",
    },
  },
};

export default meta;
type Story = StoryObj<typeof SearchInput>;

// Sample suggestions
const sampleSuggestions = [
  {
    id: "1",
    text: "React",
    description: "A JavaScript library for building user interfaces",
    icon: <Icon name="code" size={16} />,
    category: "Frameworks",
  },
  {
    id: "2",
    text: "Vue.js",
    description: "A progressive JavaScript framework",
    icon: <Icon name="code" size={16} />,
    category: "Frameworks",
  },
  {
    id: "3",
    text: "Angular",
    description: "A platform for building mobile and desktop web applications",
    icon: <Icon name="code" size={16} />,
    category: "Frameworks",
  },
  {
    id: "4",
    text: "TypeScript",
    description: "A typed superset of JavaScript",
    icon: <Icon name="file-text" size={16} />,
    category: "Languages",
  },
  {
    id: "5",
    text: "JavaScript",
    description: "A programming language for the web",
    icon: <Icon name="file-text" size={16} />,
    category: "Languages",
  },
  {
    id: "6",
    text: "Python",
    description: "A high-level programming language",
    icon: <Icon name="file-text" size={16} />,
    category: "Languages",
  },
  {
    id: "7",
    text: "Node.js",
    description: "A JavaScript runtime built on Chrome's V8 engine",
    icon: <Icon name="server" size={16} />,
    category: "Backend",
  },
  {
    id: "8",
    text: "Express",
    description: "A minimal and flexible Node.js web application framework",
    icon: <Icon name="server" size={16} />,
    category: "Backend",
  },
  {
    id: "9",
    text: "MongoDB",
    description: "A NoSQL database program",
    icon: <Icon name="database" size={16} />,
    category: "Database",
  },
  {
    id: "10",
    text: "PostgreSQL",
    description: "An open-source relational database",
    icon: <Icon name="database" size={16} />,
    category: "Database",
  },
];

export const Default: Story = {
  args: {
    suggestions: sampleSuggestions,
    placeholder: "Search...",
  },
};

export const WithLabel: Story = {
  args: {
    label: "Search Technologies",
    suggestions: sampleSuggestions,
    placeholder: "Search for technologies...",
    required: true,
  },
};

export const WithCategories: Story = {
  args: {
    label: "Search with Categories",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    showCategories: true,
  },
};

export const WithoutCategories: Story = {
  args: {
    label: "Search without Categories",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    showCategories: false,
  },
};

export const WithLoading: Story = {
  args: {
    label: "Search with Loading",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    loading: true,
  },
};

export const WithoutClear: Story = {
  args: {
    label: "Search without Clear",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    clearable: false,
  },
};

export const WithoutSuggestions: Story = {
  args: {
    label: "Search without Suggestions",
    suggestions: [],
    placeholder: "Search...",
    showSuggestions: false,
  },
};

export const Small: Story = {
  args: {
    label: "Small Search",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    size: "small",
  },
};

export const Large: Story = {
  args: {
    label: "Large Search",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    size: "large",
  },
};

export const Disabled: Story = {
  args: {
    label: "Disabled Search",
    suggestions: sampleSuggestions,
    placeholder: "This is disabled",
    disabled: true,
  },
};

export const Error: Story = {
  args: {
    label: "Search with Error",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    error: true,
    errorMessage: "Please enter a valid search term",
  },
};

export const WithCustomDebounce: Story = {
  args: {
    label: "Search with Custom Debounce",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    debounceMs: 500,
  },
};

export const WithMinLength: Story = {
  args: {
    label: "Search with Min Length",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    minLength: 3,
  },
};

export const WithMaxSuggestions: Story = {
  args: {
    label: "Search with Max Suggestions",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    maxSuggestions: 5,
  },
};

export const WithHotkey: Story = {
  args: {
    label: "Search with Hotkey",
    suggestions: sampleSuggestions,
    placeholder: "Search...",
    hotkey: "k",
  },
};

export const Controlled: Story = {
  render: () => {
    const [searchValue, setSearchValue] = useState("");
    const [searchResults, setSearchResults] = useState<string[]>([]);

    const handleSearch = (query: string) => {
      console.log("Searching for:", query);
      // Simulate search results
      const results = sampleSuggestions
        .filter(
          (suggestion) =>
            suggestion.text.toLowerCase().includes(query.toLowerCase()) ||
            suggestion.description.toLowerCase().includes(query.toLowerCase()),
        )
        .map((suggestion) => suggestion.text);
      setSearchResults(results);
    };

    const handleSelect = (suggestion: any) => {
      console.log("Selected:", suggestion);
      setSearchValue(suggestion.text);
    };

    return (
      <div>
        <SearchInput
          label="Controlled Search"
          value={searchValue}
          onChange={setSearchValue}
          onSearch={handleSearch}
          onSelect={handleSelect}
          suggestions={sampleSuggestions}
          placeholder="Search technologies..."
          showCategories={true}
        />
        <div style={{ marginTop: "16px", color: "#FFFFFF" }}>
          <h4>Search Results ({searchResults.length}):</h4>
          <ul>
            {searchResults.map((result, index) => (
              <li key={index}>{result}</li>
            ))}
          </ul>
        </div>
      </div>
    );
  },
};

export const CommandSearch: Story = {
  render: () => {
    const commandSuggestions = [
      {
        id: "1",
        text: "Create new file",
        description: "Create a new file in the current directory",
        icon: <Icon name="file-plus" size={16} />,
        category: "File",
      },
      {
        id: "2",
        text: "Open file",
        description: "Open an existing file",
        icon: <Icon name="folder-open" size={16} />,
        category: "File",
      },
      {
        id: "3",
        text: "Save file",
        description: "Save the current file",
        icon: <Icon name="save" size={16} />,
        category: "File",
      },
      {
        id: "4",
        text: "Cut",
        description: "Cut selected text",
        icon: <Icon name="scissors" size={16} />,
        category: "Edit",
      },
      {
        id: "5",
        text: "Copy",
        description: "Copy selected text",
        icon: <Icon name="copy" size={16} />,
        category: "Edit",
      },
      {
        id: "6",
        text: "Paste",
        description: "Paste from clipboard",
        icon: <Icon name="clipboard" size={16} />,
        category: "Edit",
      },
      {
        id: "7",
        text: "Search",
        description: "Search in current file",
        icon: <Icon name="search" size={16} />,
        category: "View",
      },
      {
        id: "8",
        text: "Replace",
        description: "Find and replace text",
        icon: <Icon name="replace" size={16} />,
        category: "View",
      },
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Command Search
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
            Ctrl+K
          </kbd>{" "}
          to focus the search
        </p>
        <SearchInput
          suggestions={commandSuggestions}
          placeholder="Search commands..."
          showCategories={true}
          hotkey="k"
        />
      </div>
    );
  },
};

export const UserSearch: Story = {
  render: () => {
    const userSuggestions = [
      {
        id: "1",
        text: "John Doe",
        description: "Software Engineer • john@example.com",
        icon: <Icon name="user" size={16} />,
        category: "Users",
      },
      {
        id: "2",
        text: "Jane Smith",
        description: "Product Manager • jane@example.com",
        icon: <Icon name="user" size={16} />,
        category: "Users",
      },
      {
        id: "3",
        text: "Bob Johnson",
        description: "Designer • bob@example.com",
        icon: <Icon name="user" size={16} />,
        category: "Users",
      },
      {
        id: "4",
        text: "Alice Brown",
        description: "QA Engineer • alice@example.com",
        icon: <Icon name="user" size={16} />,
        category: "Users",
      },
      {
        id: "5",
        text: "Charlie Wilson",
        description: "DevOps Engineer • charlie@example.com",
        icon: <Icon name="user" size={16} />,
        category: "Users",
      },
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>User Search</h3>
        <SearchInput
          label="Search Users"
          suggestions={userSuggestions}
          placeholder="Search for users..."
          showCategories={true}
          maxSuggestions={10}
        />
      </div>
    );
  },
};
