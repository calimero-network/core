import type { Meta, StoryObj } from "@storybook/react-vite";
import { DatePicker } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof DatePicker> = {
  title: "Forms/Date Picker",
  component: DatePicker,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    value: {
      control: { type: "date" },
      description: "Selected date",
    },
    placeholder: {
      control: { type: "text" },
      description: "Input placeholder",
    },
    disabled: {
      control: { type: "boolean" },
      description: "Disable the picker",
    },
    showTime: {
      control: { type: "boolean" },
      description: "Show time picker",
    },
    timeFormat: {
      control: { type: "select" },
      options: ["12h", "24h"],
      description: "Time format",
    },
    minDate: {
      control: { type: "date" },
      description: "Minimum selectable date",
    },
    maxDate: {
      control: { type: "date" },
      description: "Maximum selectable date",
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
  },
};

export default meta;
type Story = StoryObj<typeof DatePicker>;

export const Default: Story = {
  args: {
    placeholder: "Select a date",
  },
};

export const WithLabel: Story = {
  args: {
    label: "Birth Date",
    placeholder: "Select your birth date",
    required: true,
  },
};

export const WithTime: Story = {
  args: {
    label: "Event Date & Time",
    placeholder: "Select date and time",
    showTime: true,
    timeFormat: "12h",
  },
};

export const WithTime24h: Story = {
  args: {
    label: "Meeting Time",
    placeholder: "Select date and time",
    showTime: true,
    timeFormat: "24h",
  },
};

export const WithMinMax: Story = {
  args: {
    label: "Appointment Date",
    placeholder: "Select appointment date",
    minDate: new Date(),
    maxDate: new Date(Date.now() + 30 * 24 * 60 * 60 * 1000), // 30 days from now
  },
};

export const Small: Story = {
  args: {
    label: "Small Date Picker",
    size: "small",
    placeholder: "Select date",
  },
};

export const Large: Story = {
  args: {
    label: "Large Date Picker",
    size: "large",
    placeholder: "Select date",
  },
};

export const Disabled: Story = {
  args: {
    label: "Disabled Date Picker",
    placeholder: "This is disabled",
    disabled: true,
  },
};

export const Error: Story = {
  args: {
    label: "Date with Error",
    placeholder: "Select date",
    error: true,
    errorMessage: "Please select a valid date",
  },
};

export const Controlled: Story = {
  render: () => {
    const [date, setDate] = useState<Date | null>(null);

    return (
      <div>
        <DatePicker
          label="Controlled Date Picker"
          value={date}
          onChange={setDate}
          placeholder="Select a date"
          showTime={true}
        />
        <div style={{ marginTop: "16px", color: "#FFFFFF" }}>
          Selected date: {date ? date.toLocaleString() : "None"}
        </div>
      </div>
    );
  },
};

export const FormExample: Story = {
  render: () => {
    const [formData, setFormData] = useState({
      startDate: null as Date | null,
      endDate: null as Date | null,
      eventTime: null as Date | null,
    });

    const handleSubmit = (e: React.FormEvent) => {
      e.preventDefault();
      alert(
        `Form submitted with:\nStart: ${formData.startDate?.toLocaleDateString()}\nEnd: ${formData.endDate?.toLocaleDateString()}\nEvent: ${formData.eventTime?.toLocaleString()}`,
      );
    };

    return (
      <form onSubmit={handleSubmit} style={{ maxWidth: "400px" }}>
        <h3 style={{ color: "#FFFFFF", marginBottom: "20px" }}>Event Form</h3>

        <div style={{ marginBottom: "16px" }}>
          <DatePicker
            label="Start Date"
            value={formData.startDate}
            onChange={(date) =>
              setFormData((prev) => ({ ...prev, startDate: date }))
            }
            placeholder="Select start date"
            required
          />
        </div>

        <div style={{ marginBottom: "16px" }}>
          <DatePicker
            label="End Date"
            value={formData.endDate}
            onChange={(date) =>
              setFormData((prev) => ({ ...prev, endDate: date }))
            }
            placeholder="Select end date"
            minDate={formData.startDate || undefined}
            required
          />
        </div>

        <div style={{ marginBottom: "20px" }}>
          <DatePicker
            label="Event Date & Time"
            value={formData.eventTime}
            onChange={(date) =>
              setFormData((prev) => ({ ...prev, eventTime: date }))
            }
            placeholder="Select event date and time"
            showTime={true}
            timeFormat="12h"
            required
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
          Submit Event
        </button>
      </form>
    );
  },
};
