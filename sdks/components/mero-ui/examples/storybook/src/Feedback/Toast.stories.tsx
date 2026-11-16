import type { Meta, StoryObj } from "@storybook/react-vite";
import { ToastProvider, useToast, Button } from "@calimero-network/mero-ui";
import React from "react";

const meta: Meta<typeof ToastProvider> = {
  title: "Feedback/Toast",
  component: ToastProvider,
  parameters: { layout: "fullscreen" },
};

export default meta;

type Story = StoryObj<typeof ToastProvider>;

function ToastDemo() {
  const toast = useToast();

  return (
    <div
      style={{
        padding: "20px",
        display: "flex",
        gap: "12px",
        flexWrap: "wrap",
      }}
    >
      <Button
        onClick={() =>
          toast.show({
            title: "Success!",
            description: "Your changes have been saved.",
            variant: "success",
          })
        }
      >
        Success Toast
      </Button>
      <Button
        onClick={() =>
          toast.show({
            title: "Error",
            description: "Something went wrong.",
            variant: "error",
          })
        }
      >
        Error Toast
      </Button>
      <Button
        onClick={() =>
          toast.show({
            title: "Warning",
            description: "Please check your input.",
            variant: "warning",
          })
        }
      >
        Warning Toast
      </Button>
      <Button
        onClick={() =>
          toast.show({
            title: "Info",
            description: "Here is some information.",
            variant: "info",
          })
        }
      >
        Info Toast
      </Button>
      <Button
        onClick={() =>
          toast.show({
            title: "Default Toast",
            description: "This is a default toast.",
          })
        }
      >
        Default Toast
      </Button>
      <Button onClick={() => toast.clear()}>Clear All</Button>
    </div>
  );
}

export const Default: Story = {
  render: () => (
    <ToastProvider>
      <ToastDemo />
    </ToastProvider>
  ),
};
