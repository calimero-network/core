import type { Meta, StoryObj } from "@storybook/react-vite";
import { Select } from "@calimero-network/mero-ui";

const meta: Meta<typeof Select> = {
  title: "Forms/Select",
  component: Select,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Select>;

const options = [
  { value: "option1", label: "Option 1" },
  { value: "option2", label: "Option 2" },
  { value: "option3", label: "Option 3", disabled: true },
  { value: "option4", label: "Option 4" },
];

export const Default: Story = {
  args: {
    options,
    placeholder: "Select an option...",
  },
};

export const WithLabel: Story = {
  args: {
    label: "Choose a plan",
    options,
    placeholder: "Select a plan...",
  },
};

export const WithDescription: Story = {
  args: {
    label: "Country",
    description: "Select your country of residence",
    options: [
      { value: "us", label: "United States" },
      { value: "ca", label: "Canada" },
      { value: "uk", label: "United Kingdom" },
      { value: "de", label: "Germany" },
    ],
  },
};

export const WithError: Story = {
  args: {
    label: "Required field",
    options,
    error: "This field is required",
  },
};

export const Disabled: Story = {
  args: {
    label: "Disabled select",
    options,
    disabled: true,
  },
};
