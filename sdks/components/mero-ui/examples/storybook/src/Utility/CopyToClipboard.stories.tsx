import type { Meta, StoryObj } from "@storybook/react-vite";
import { CopyToClipboard, useCopyToClipboard } from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof CopyToClipboard> = {
  title: "Utility/Copy to Clipboard",
  component: CopyToClipboard,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    text: {
      control: { type: "text" },
      description: "Text to copy to clipboard",
    },
    variant: {
      control: { type: "select" },
      options: ["button", "icon", "text", "custom"],
      description: "Display variant",
    },
    size: {
      control: { type: "select" },
      options: ["small", "medium", "large"],
      description: "Component size",
    },
    disabled: {
      control: { type: "boolean" },
      description: "Disable the copy button",
    },
    showToast: {
      control: { type: "boolean" },
      description: "Show toast notification",
    },
    successMessage: {
      control: { type: "text" },
      description: "Success message",
    },
    errorMessage: {
      control: { type: "text" },
      description: "Error message",
    },
    showFeedback: {
      control: { type: "boolean" },
      description: "Show visual feedback",
    },
    feedbackDuration: {
      control: { type: "number" },
      description: "Feedback duration in milliseconds",
    },
  },
};

export default meta;
type Story = StoryObj<typeof CopyToClipboard>;

export const Default: Story = {
  args: {
    text: "Hello, World!",
  },
};

export const ButtonVariant: Story = {
  args: {
    text: "Copy this text",
    variant: "button",
  },
};

export const IconVariant: Story = {
  args: {
    text: "Copy this text",
    variant: "icon",
  },
};

export const TextVariant: Story = {
  args: {
    text: "Copy this text",
    variant: "text",
  },
};

export const CustomVariant: Story = {
  args: {
    text: "Copy this text",
    variant: "custom",
    children: (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "8px",
          padding: "8px 12px",
          background: "var(--color-background-secondary)",
          borderRadius: "6px",
          cursor: "pointer",
          border: "1px solid var(--color-neutral-600)",
          color: "#FFFFFF",
        }}
      >
        <Icon name="copy" size={16} />
        <span>Custom Copy Button</span>
      </div>
    ),
  },
};

export const Small: Story = {
  args: {
    text: "Small copy button",
    size: "small",
  },
};

export const Large: Story = {
  args: {
    text: "Large copy button",
    size: "large",
  },
};

export const Disabled: Story = {
  args: {
    text: "This is disabled",
    disabled: true,
  },
};

export const WithoutToast: Story = {
  args: {
    text: "No toast notification",
    showToast: false,
  },
};

export const CustomMessages: Story = {
  args: {
    text: "Custom success message",
    successMessage: "Text copied successfully!",
    errorMessage: "Failed to copy text. Please try again.",
  },
};

export const WithoutFeedback: Story = {
  args: {
    text: "No visual feedback",
    showFeedback: false,
  },
};

export const LongText: Story = {
  args: {
    text: "This is a very long text that will be copied to the clipboard. It contains multiple sentences and demonstrates how the copy to clipboard component handles longer content. The text can be of any length and will be copied in its entirety.",
  },
};

export const CodeSnippet: Story = {
  args: {
    text: `function hello() {
  console.log("Hello, World!");
  return "success";
}`,
    variant: "custom",
    children: (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "8px",
          padding: "8px 12px",
          background: "var(--color-background-secondary)",
          borderRadius: "6px",
          cursor: "pointer",
          border: "1px solid var(--color-neutral-600)",
          color: "#FFFFFF",
          fontFamily: "monospace",
          fontSize: "14px",
        }}
      >
        <Icon name="code" size={16} />
        <span>Copy Code</span>
      </div>
    ),
  },
};

export const URL: Story = {
  args: {
    text: "https://example.com/very-long-url-that-might-be-useful-to-copy",
    variant: "custom",
    children: (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "8px",
          padding: "8px 12px",
          background: "var(--color-background-secondary)",
          borderRadius: "6px",
          cursor: "pointer",
          border: "1px solid var(--color-neutral-600)",
          color: "var(--color-brand-600)",
          fontSize: "14px",
        }}
      >
        <Icon name="link" size={16} />
        <span>Copy URL</span>
      </div>
    ),
  },
};

export const WithHook: Story = {
  render: () => {
    const { copy, status } = useCopyToClipboard();
    const [text, setText] = useState("Hello from the hook!");

    const handleCopy = async () => {
      const success = await copy(text);
      console.log("Copy result:", success);
    };

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Using the Hook
        </h3>
        <div style={{ marginBottom: "16px" }}>
          <input
            type="text"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Enter text to copy"
            style={{
              width: "100%",
              maxWidth: "400px",
              padding: "8px 12px",
              background: "var(--color-background-secondary)",
              border: "1px solid var(--color-neutral-600)",
              borderRadius: "6px",
              color: "#FFFFFF",
              marginBottom: "12px",
            }}
          />
        </div>
        <button
          onClick={handleCopy}
          style={{
            padding: "8px 16px",
            background:
              status === "success"
                ? "var(--color-semantic-success)"
                : status === "error"
                  ? "var(--color-semantic-error)"
                  : "var(--color-brand-600)",
            color:
              status === "success" || status === "error"
                ? "#FFFFFF"
                : "#000000",
            border: "none",
            borderRadius: "6px",
            cursor: "pointer",
            fontWeight: "500",
          }}
        >
          {status === "success"
            ? "Copied!"
            : status === "error"
              ? "Error"
              : "Copy Text"}
        </button>
        <div style={{ marginTop: "12px", color: "#FFFFFF", fontSize: "14px" }}>
          Status: {status}
        </div>
      </div>
    );
  },
};

export const MultipleInstances: Story = {
  render: () => {
    const items = [
      { id: 1, text: "First item to copy", label: "Item 1" },
      { id: 2, text: "Second item to copy", label: "Item 2" },
      { id: 3, text: "Third item to copy", label: "Item 3" },
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Multiple Copy Buttons
        </h3>
        <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
          {items.map((item) => (
            <div
              key={item.id}
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                padding: "12px",
                background: "var(--color-background-secondary)",
                borderRadius: "6px",
                border: "1px solid var(--color-neutral-600)",
              }}
            >
              <span style={{ color: "#FFFFFF" }}>
                {item.label}: {item.text}
              </span>
              <CopyToClipboard text={item.text} variant="icon" size="small" />
            </div>
          ))}
        </div>
      </div>
    );
  },
};

export const FormExample: Story = {
  render: () => {
    const [formData, setFormData] = useState({
      name: "John Doe",
      email: "john@example.com",
      phone: "+1 (555) 123-4567",
      address: "123 Main St, Anytown, USA 12345",
    });

    const copyAllData = () => {
      const dataString = Object.entries(formData)
        .map(([key, value]) => `${key}: ${value}`)
        .join("\n");
      return dataString;
    };

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "20px" }}>
          Form Data Copy Example
        </h3>
        <div
          style={{
            background: "var(--color-background-secondary)",
            padding: "20px",
            borderRadius: "8px",
            border: "1px solid var(--color-neutral-600)",
          }}
        >
          <div style={{ marginBottom: "16px" }}>
            <label
              style={{
                display: "block",
                color: "#FFFFFF",
                marginBottom: "4px",
              }}
            >
              Name:
            </label>
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <span style={{ color: "#FFFFFF" }}>{formData.name}</span>
              <CopyToClipboard
                text={formData.name}
                variant="icon"
                size="small"
              />
            </div>
          </div>

          <div style={{ marginBottom: "16px" }}>
            <label
              style={{
                display: "block",
                color: "#FFFFFF",
                marginBottom: "4px",
              }}
            >
              Email:
            </label>
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <span style={{ color: "#FFFFFF" }}>{formData.email}</span>
              <CopyToClipboard
                text={formData.email}
                variant="icon"
                size="small"
              />
            </div>
          </div>

          <div style={{ marginBottom: "16px" }}>
            <label
              style={{
                display: "block",
                color: "#FFFFFF",
                marginBottom: "4px",
              }}
            >
              Phone:
            </label>
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <span style={{ color: "#FFFFFF" }}>{formData.phone}</span>
              <CopyToClipboard
                text={formData.phone}
                variant="icon"
                size="small"
              />
            </div>
          </div>

          <div style={{ marginBottom: "20px" }}>
            <label
              style={{
                display: "block",
                color: "#FFFFFF",
                marginBottom: "4px",
              }}
            >
              Address:
            </label>
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <span style={{ color: "#FFFFFF" }}>{formData.address}</span>
              <CopyToClipboard
                text={formData.address}
                variant="icon"
                size="small"
              />
            </div>
          </div>

          <div
            style={{
              borderTop: "1px solid var(--color-neutral-600)",
              paddingTop: "16px",
            }}
          >
            <CopyToClipboard
              text={copyAllData()}
              variant="button"
              successMessage="All form data copied!"
            >
              <div
                style={{ display: "flex", alignItems: "center", gap: "8px" }}
              >
                <Icon name="copy" size={16} />
                Copy All Data
              </div>
            </CopyToClipboard>
          </div>
        </div>
      </div>
    );
  },
};
