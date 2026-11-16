import type { Meta, StoryObj } from "@storybook/react-vite";
import React from "react";
import { SearchHighlight } from "@calimero-network/mero-ui";

const meta: Meta<typeof SearchHighlight> = {
  title: "Utility/Search Highlight",
  component: SearchHighlight,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    text: {
      control: { type: "text" },
      description: "Text to search within",
    },
    searchTerm: {
      control: { type: "text" },
      description: "Search term to highlight",
    },
    highlightColor: {
      control: { type: "color" },
      description: "Color for highlighting",
    },
    caseSensitive: {
      control: { type: "boolean" },
      description: "Whether search is case sensitive",
    },
  },
};

export default meta;
type Story = StoryObj<typeof SearchHighlight>;

export const Default: Story = {
  args: {
    text: "This is a sample text with some search terms to highlight.",
    searchTerm: "search",
    caseSensitive: false,
  },
};

export const CaseSensitive: Story = {
  args: {
    text: "This is a Sample text with some Search terms to highlight.",
    searchTerm: "Sample",
    caseSensitive: true,
  },
};

export const MultipleMatches: Story = {
  args: {
    text: "The quick brown fox jumps over the lazy dog. The fox is quick and brown.",
    searchTerm: "fox",
    caseSensitive: false,
  },
};

export const LongText: Story = {
  args: {
    text: "React is a JavaScript library for building user interfaces. It is maintained by Facebook and a community of individual developers and companies. React can be used as a base in the development of single-page or mobile applications.",
    searchTerm: "React",
    caseSensitive: false,
  },
};

export const CustomColor: Story = {
  args: {
    text: "This text uses a custom highlight color for the search term.",
    searchTerm: "custom",
    highlightColor: "#ff6b6b",
    caseSensitive: false,
  },
};

export const NoMatch: Story = {
  args: {
    text: "This text has no matching search terms.",
    searchTerm: "nonexistent",
    caseSensitive: false,
  },
};

export const EmptySearch: Story = {
  args: {
    text: "This text will not be highlighted because search term is empty.",
    searchTerm: "",
    caseSensitive: false,
  },
};

export const SpecialCharacters: Story = {
  args: {
    text: "This text contains special characters like [brackets], (parentheses), and {braces}.",
    searchTerm: "[brackets]",
    caseSensitive: false,
  },
};

export const Interactive: Story = {
  render: () => {
    const [searchTerm, setSearchTerm] = React.useState("highlight");
    const text =
      "This is an interactive example where you can change the search term to see different highlighting results.";

    return (
      <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
        <input
          type="text"
          value={searchTerm}
          onChange={(e) => setSearchTerm(e.target.value)}
          placeholder="Enter search term"
          style={{
            padding: "8px 12px",
            border: "1px solid #404040",
            borderRadius: "6px",
            background: "var(--color-background-primary)",
            color: "#FFFFFF",
          }}
        />
        <div style={{ fontSize: "16px", lineHeight: "1.5" }}>
          <SearchHighlight text={text} searchTerm={searchTerm} />
        </div>
      </div>
    );
  },
};
