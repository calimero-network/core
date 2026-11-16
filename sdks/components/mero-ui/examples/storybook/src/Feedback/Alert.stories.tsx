import type { Meta, StoryObj } from "@storybook/react-vite";
import { Alert, Button } from "@calimero-network/mero-ui";
import React from "react";

const meta: Meta<typeof Alert> = {
  title: "Feedback/Alert",
  component: Alert,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Alert>;

export const Info: Story = {
  args: {
    variant: "info",
    title: "Heads up",
    description: "This is an informational message.",
  },
};

export const Success: Story = {
  args: {
    variant: "success",
    title: "All set",
    description: "Your changes have been saved.",
  },
};

export const Warning: Story = {
  args: {
    variant: "warning",
    title: "Be careful",
    description: "This action cannot be undone.",
  },
};

export const Error: Story = {
  args: {
    variant: "error",
    title: "Something went wrong",
    description: "Please try again later.",
  },
};

export const Dismissible: Story = {
  render: () => {
    const [open, setOpen] = React.useState(true);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        {open && (
          <Alert
            variant="info"
            title="Notice"
            description="You can close this alert."
            dismissible
            onClose={() => setOpen(false)}
          />
        )}
        {!open && <Button onClick={() => setOpen(true)}>Re-open</Button>}
      </div>
    );
  },
};
