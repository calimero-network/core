import type { Meta, StoryObj } from "@storybook/react-vite";
import { Modal, Button } from "@calimero-network/mero-ui";
import React from "react";

const meta: Meta<typeof Modal> = {
  title: "Feedback/Modal",
  component: Modal,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Modal>;

export const Basic: Story = {
  render: () => {
    const [open, setOpen] = React.useState(false);
    return (
      <div
        style={{
          height: 300,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <Button onClick={() => setOpen(true)}>Open Modal</Button>
        <Modal open={open} onClose={() => setOpen(false)} title="Example modal">
          <p>Modal content goes here.</p>
        </Modal>
      </div>
    );
  },
};
