import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Accordion,
  AccordionItem,
  AccordionItemControlled,
} from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof Accordion> = {
  title: "Navigation/Accordion",
  component: Accordion,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    items: {
      control: { type: "object" },
      description: "Array of accordion items",
    },
    allowMultiple: {
      control: { type: "boolean" },
      description: "Allow multiple items to be open",
    },
    variant: {
      control: { type: "select" },
      options: ["default", "bordered", "filled"],
      description: "Accordion variant",
    },
    size: {
      control: { type: "select" },
      options: ["small", "medium", "large"],
      description: "Accordion size",
    },
  },
};

export default meta;
type Story = StoryObj<typeof Accordion>;

const sampleItems = [
  {
    title: "What is this product?",
    children:
      "This is a comprehensive design system built with React and TypeScript. It provides a complete set of reusable components, design tokens, and utilities to help you build beautiful and consistent user interfaces.",
    icon: <Icon name="help-circle" size={20} />,
  },
  {
    title: "How do I get started?",
    children:
      "Getting started is easy! Simply install the package, import the components you need, and start building. Check out our documentation for detailed setup instructions and examples.",
    icon: <Icon name="play-circle" size={20} />,
  },
  {
    title: "What components are available?",
    children:
      "We provide a wide range of components including forms, data display, navigation, feedback, and utility components. Each component is fully customizable and follows accessibility best practices.",
    icon: <Icon name="grid" size={20} />,
  },
  {
    title: "Is it accessible?",
    children:
      "Yes! All components are built with accessibility in mind. They follow WCAG 2.1 guidelines and include proper ARIA attributes, keyboard navigation, and screen reader support.",
    icon: <Icon name="accessibility" size={20} />,
  },
  {
    title: "Can I customize the styling?",
    children:
      "Absolutely! The design system is built with customization in mind. You can override styles, use custom themes, and even extend components to fit your specific needs.",
    icon: <Icon name="palette" size={20} />,
  },
];

export const Default: Story = {
  args: {
    items: sampleItems,
  },
};

export const SingleOpen: Story = {
  args: {
    items: sampleItems,
    allowMultiple: false,
  },
};

export const MultipleOpen: Story = {
  args: {
    items: sampleItems,
    allowMultiple: true,
  },
};

export const Bordered: Story = {
  args: {
    items: sampleItems,
    variant: "bordered",
  },
};

export const Filled: Story = {
  args: {
    items: sampleItems,
    variant: "filled",
  },
};

export const Small: Story = {
  args: {
    items: sampleItems,
    size: "small",
  },
};

export const Large: Story = {
  args: {
    items: sampleItems,
    size: "large",
  },
};

export const WithoutIcons: Story = {
  args: {
    items: sampleItems.map((item) => ({ ...item, icon: undefined })),
  },
};

export const WithDisabledItems: Story = {
  args: {
    items: [
      ...sampleItems.slice(0, 2),
      {
        ...sampleItems[2],
        disabled: true,
      },
      ...sampleItems.slice(3),
    ],
  },
};

export const IndividualItems: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
        Individual Accordion Items
      </h3>
      <AccordionItem
        title="First Item"
        icon={<Icon name="star" size={20} />}
        defaultOpen={true}
      >
        <p>
          This is the content of the first accordion item. It can contain any
          React elements.
        </p>
        <ul style={{ marginTop: "12px", paddingLeft: "20px" }}>
          <li>List item 1</li>
          <li>List item 2</li>
          <li>List item 3</li>
        </ul>
      </AccordionItem>

      <AccordionItem title="Second Item" icon={<Icon name="heart" size={20} />}>
        <p>This is the content of the second accordion item.</p>
        <div
          style={{
            marginTop: "12px",
            padding: "12px",
            background: "var(--color-background-secondary)",
            borderRadius: "6px",
          }}
        >
          <strong>Note:</strong> You can include any content here, including
          other components.
        </div>
      </AccordionItem>

      <AccordionItem
        title="Third Item"
        icon={<Icon name="settings" size={20} />}
      >
        <p>This is the content of the third accordion item.</p>
      </AccordionItem>
    </div>
  ),
};

export const ControlledItems: Story = {
  render: () => {
    const [openItems, setOpenItems] = useState<Set<number>>(new Set([0]));

    const handleToggle = (index: number) => {
      setOpenItems((prev) => {
        const newOpenItems = new Set(prev);
        if (newOpenItems.has(index)) {
          newOpenItems.delete(index);
        } else {
          newOpenItems.add(index);
        }
        return newOpenItems;
      });
    };

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Controlled Accordion Items
        </h3>
        <p style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Open items: {Array.from(openItems).join(", ") || "None"}
        </p>

        {sampleItems.map((item, index) => (
          <AccordionItemControlled
            key={index}
            title={item.title}
            icon={item.icon}
            isOpen={openItems.has(index)}
            onToggle={() => handleToggle(index)}
          >
            {item.children}
          </AccordionItemControlled>
        ))}
      </div>
    );
  },
};

export const FAQExample: Story = {
  render: () => {
    const faqItems = [
      {
        title: "How do I install the design system?",
        children: (
          <div>
            <p>You can install the design system using npm or yarn:</p>
            <pre
              style={{
                background: "var(--color-background-secondary)",
                padding: "12px",
                borderRadius: "6px",
                marginTop: "8px",
                fontSize: "14px",
                color: "#FFFFFF",
              }}
            >
              {`npm install @calimero-network/mero-ui
# or
yarn add @calimero-network/mero-ui`}
            </pre>
          </div>
        ),
        icon: <Icon name="download" size={20} />,
      },
      {
        title: "What browsers are supported?",
        children: (
          <div>
            <p>Our design system supports all modern browsers:</p>
            <ul style={{ marginTop: "8px", paddingLeft: "20px" }}>
              <li>Chrome 90+</li>
              <li>Firefox 88+</li>
              <li>Safari 14+</li>
              <li>Edge 90+</li>
            </ul>
          </div>
        ),
        icon: <Icon name="globe" size={20} />,
      },
      {
        title: "How do I customize the theme?",
        children: (
          <div>
            <p>
              You can customize the theme by overriding CSS variables or using
              our theme provider:
            </p>
            <pre
              style={{
                background: "var(--color-background-secondary)",
                padding: "12px",
                borderRadius: "6px",
                marginTop: "8px",
                fontSize: "14px",
                color: "#FFFFFF",
              }}
            >
              {`import { ThemeProvider } from '@calimero-network/mero-ui';

<ThemeProvider theme={customTheme}>
  <App />
</ThemeProvider>`}
            </pre>
          </div>
        ),
        icon: <Icon name="palette" size={20} />,
      },
      {
        title: "Is there TypeScript support?",
        children: (
          <div>
            <p>
              Yes! The entire design system is built with TypeScript and
              includes full type definitions for all components and props.
            </p>
          </div>
        ),
        icon: <Icon name="code" size={20} />,
      },
    ];

    return (
      <div>
        <h2 style={{ color: "#FFFFFF", marginBottom: "24px" }}>
          Frequently Asked Questions
        </h2>
        <Accordion items={faqItems} allowMultiple={true} />
      </div>
    );
  },
};
