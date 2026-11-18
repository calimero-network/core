import type { Meta, StoryObj } from "@storybook/react-vite";
import { TimePicker } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof TimePicker> = {
  title: "Forms/Time Picker",
  component: TimePicker,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    value: {
      control: { type: "date" },
      description: "Selected time",
    },
    placeholder: {
      control: { type: "text" },
      description: "Input placeholder",
    },
    disabled: {
      control: { type: "boolean" },
      description: "Disable the picker",
    },
    format: {
      control: { type: "select" },
      options: ["12h", "24h"],
      description: "Time format",
    },
    minTime: {
      control: { type: "text" },
      description: "Minimum selectable time (HH:mm)",
    },
    maxTime: {
      control: { type: "text" },
      description: "Maximum selectable time (HH:mm)",
    },
    step: {
      control: { type: "number" },
      description: "Minutes step",
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
    showSeconds: {
      control: { type: "boolean" },
      description: "Show seconds picker",
    },
    use12Hours: {
      control: { type: "boolean" },
      description: "Use 12-hour format",
    },
  },
};

export default meta;
type Story = StoryObj<typeof TimePicker>;

export const Default: Story = {
  args: {
    placeholder: "Select time",
  },
};

export const WithLabel: Story = {
  args: {
    label: "Start Time",
    placeholder: "Select start time",
    required: true,
  },
};

export const TwelveHour: Story = {
  args: {
    label: "Meeting Time",
    placeholder: "Select meeting time",
    format: "12h",
    use12Hours: true,
  },
};

export const TwentyFourHour: Story = {
  args: {
    label: "Event Time",
    placeholder: "Select event time",
    format: "24h",
    use12Hours: false,
  },
};

export const WithSeconds: Story = {
  args: {
    label: "Precise Time",
    placeholder: "Select precise time",
    showSeconds: true,
    use12Hours: false,
  },
};

export const WithMinMax: Story = {
  args: {
    label: "Business Hours",
    placeholder: "Select business time",
    minTime: "09:00",
    maxTime: "17:00",
  },
};

export const WithStep: Story = {
  args: {
    label: "Appointment Time",
    placeholder: "Select appointment time",
    step: 30,
  },
};

export const Small: Story = {
  args: {
    label: "Small Time Picker",
    size: "small",
    placeholder: "Select time",
  },
};

export const Large: Story = {
  args: {
    label: "Large Time Picker",
    size: "large",
    placeholder: "Select time",
  },
};

export const Disabled: Story = {
  args: {
    label: "Disabled Time Picker",
    placeholder: "This is disabled",
    disabled: true,
  },
};

export const Error: Story = {
  args: {
    label: "Time with Error",
    placeholder: "Select time",
    error: true,
    errorMessage: "Please select a valid time",
  },
};

export const Controlled: Story = {
  render: () => {
    const [time, setTime] = useState<Date | null>(null);

    return (
      <div>
        <TimePicker
          label="Controlled Time Picker"
          value={time}
          onChange={setTime}
          placeholder="Select a time"
          showSeconds={true}
        />
        <div style={{ marginTop: "16px", color: "#FFFFFF" }}>
          Selected time: {time ? time.toLocaleTimeString() : "None"}
        </div>
      </div>
    );
  },
};

export const FormExample: Story = {
  render: () => {
    const [formData, setFormData] = useState({
      startTime: null as Date | null,
      endTime: null as Date | null,
      breakTime: null as Date | null,
    });

    const handleSubmit = (e: React.FormEvent) => {
      e.preventDefault();
      alert(
        `Form submitted with:\nStart: ${formData.startTime?.toLocaleTimeString()}\nEnd: ${formData.endTime?.toLocaleTimeString()}\nBreak: ${formData.breakTime?.toLocaleTimeString()}`,
      );
    };

    return (
      <form onSubmit={handleSubmit} style={{ maxWidth: "400px" }}>
        <h3 style={{ color: "#FFFFFF", marginBottom: "20px" }}>
          Schedule Form
        </h3>

        <div style={{ marginBottom: "16px" }}>
          <TimePicker
            label="Start Time"
            value={formData.startTime}
            onChange={(time) =>
              setFormData((prev) => ({ ...prev, startTime: time }))
            }
            placeholder="Select start time"
            required
          />
        </div>

        <div style={{ marginBottom: "16px" }}>
          <TimePicker
            label="End Time"
            value={formData.endTime}
            onChange={(time) =>
              setFormData((prev) => ({ ...prev, endTime: time }))
            }
            placeholder="Select end time"
            minTime="09:00"
            maxTime="18:00"
            required
          />
        </div>

        <div style={{ marginBottom: "20px" }}>
          <TimePicker
            label="Break Time"
            value={formData.breakTime}
            onChange={(time) =>
              setFormData((prev) => ({ ...prev, breakTime: time }))
            }
            placeholder="Select break time"
            step={15}
          />
        </div>

        <button
          type="submit"
          style={{
            background: "var(--color-brand-600)",
            color: "#000000",
            border: "none",
            padding: "12px 24px",
            borderRadius: "6px",
            cursor: "pointer",
            fontWeight: "500",
          }}
        >
          Submit Schedule
        </button>
      </form>
    );
  },
};

export const DifferentFormats: Story = {
  render: () => (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
        gap: "20px",
      }}
    >
      <div>
        <h4 style={{ color: "#FFFFFF", marginBottom: "12px" }}>
          12-Hour Format
        </h4>
        <TimePicker
          label="12-Hour Time"
          placeholder="Select time"
          format="12h"
          use12Hours={true}
        />
      </div>

      <div>
        <h4 style={{ color: "#FFFFFF", marginBottom: "12px" }}>
          24-Hour Format
        </h4>
        <TimePicker
          label="24-Hour Time"
          placeholder="Select time"
          format="24h"
          use12Hours={false}
        />
      </div>

      <div>
        <h4 style={{ color: "#FFFFFF", marginBottom: "12px" }}>With Seconds</h4>
        <TimePicker
          label="Precise Time"
          placeholder="Select time"
          showSeconds={true}
          use12Hours={false}
        />
      </div>

      <div>
        <h4 style={{ color: "#FFFFFF", marginBottom: "12px" }}>
          30-Minute Steps
        </h4>
        <TimePicker
          label="Appointment Time"
          placeholder="Select time"
          step={30}
        />
      </div>
    </div>
  ),
};
