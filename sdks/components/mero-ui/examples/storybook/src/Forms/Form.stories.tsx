import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Form,
  Fieldset,
  FormField,
  FormGroup,
  Input,
  Button,
  Textarea,
  Select,
  Checkbox,
  RadioGroup,
  Radio,
} from "@calimero-network/mero-ui";
import React, { useState } from "react";

const meta: Meta<typeof Form> = {
  title: "Forms/Form",
  component: Form,
  parameters: {
    layout: "padded",
  },
  tags: ["autodocs"],
};

export default meta;
type Story = StoryObj<typeof Form>;

export const Default: Story = {
  render: () => {
    const [formData, setFormData] = useState({});

    const validationSchema = {
      name: (value: string) => {
        if (!value) return "Name is required";
        if (value.length < 2) return "Name must be at least 2 characters";
        return undefined;
      },
      email: (value: string) => {
        if (!value) return "Email is required";
        if (!/\S+@\S+\.\S+/.test(value)) return "Email is invalid";
        return undefined;
      },
    };

    return (
      <Form
        initialValues={{
          name: "",
          email: "",
          message: "",
          country: "",
          newsletter: false,
        }}
        validationSchema={validationSchema}
        onSubmit={(values) => {
          console.log("Form submitted:", values);
          setFormData(values);
        }}
        style={{ maxWidth: "500px" }}
      >
        <FormField name="name" label="Full Name" required>
          <Input placeholder="Enter your full name" />
        </FormField>

        <FormField name="email" label="Email Address" required>
          <Input type="email" placeholder="Enter your email" />
        </FormField>

        <FormField name="country" label="Country">
          <Select
            options={[
              { value: "us", label: "United States" },
              { value: "ca", label: "Canada" },
              { value: "uk", label: "United Kingdom" },
              { value: "de", label: "Germany" },
            ]}
            placeholder="Select a country"
          />
        </FormField>

        <FormField name="message" label="Message">
          <Textarea placeholder="Enter your message" rows={4} />
        </FormField>

        <FormField name="newsletter" label="">
          <Checkbox>Subscribe to newsletter</Checkbox>
        </FormField>

        <Button type="submit" variant="primary">
          Submit Form
        </Button>
      </Form>
    );
  },
};

export const WithFieldset: Story = {
  render: () => (
    <Form style={{ maxWidth: "500px" }}>
      <Fieldset legend="Personal Information">
        <FormField name="firstName" label="First Name" required>
          <Input placeholder="Enter first name" />
        </FormField>

        <FormField name="lastName" label="Last Name" required>
          <Input placeholder="Enter last name" />
        </FormField>
      </Fieldset>

      <Fieldset legend="Contact Information">
        <FormField name="email" label="Email" required>
          <Input type="email" placeholder="Enter email" />
        </FormField>

        <FormField name="phone" label="Phone">
          <Input type="tel" placeholder="Enter phone number" />
        </FormField>
      </Fieldset>

      <Button type="submit" variant="primary">
        Submit
      </Button>
    </Form>
  ),
};

export const FormGroupExample: Story = {
  render: () => (
    <Form style={{ maxWidth: "500px" }}>
      <FormField name="name" label="Full Name" required>
        <Input placeholder="Enter your full name" />
      </FormField>

      <FormGroup direction="row" gap="md">
        <FormField name="firstName" label="First Name" required>
          <Input placeholder="First name" />
        </FormField>

        <FormField name="lastName" label="Last Name" required>
          <Input placeholder="Last name" />
        </FormField>
      </FormGroup>

      <FormGroup direction="row" gap="sm">
        <FormField name="city" label="City">
          <Input placeholder="City" />
        </FormField>

        <FormField name="state" label="State">
          <Input placeholder="State" />
        </FormField>

        <FormField name="zip" label="ZIP">
          <Input placeholder="ZIP" />
        </FormField>
      </FormGroup>

      <Button type="submit" variant="primary">
        Submit
      </Button>
    </Form>
  ),
};

export const RadioGroupExample: Story = {
  render: () => {
    const [selectedValue, setSelectedValue] = useState("");

    return (
      <Form style={{ maxWidth: "500px" }}>
        <FormField name="preference" label="Communication Preference" required>
          <RadioGroup
            value={selectedValue}
            onChange={setSelectedValue}
            options={[
              { value: "email", label: "Email" },
              { value: "phone", label: "Phone" },
              { value: "sms", label: "SMS" },
            ]}
          />
        </FormField>

        <Button type="submit" variant="primary">
          Submit
        </Button>
      </Form>
    );
  },
};
