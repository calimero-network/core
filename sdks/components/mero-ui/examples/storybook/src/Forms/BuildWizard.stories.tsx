import type { Meta, StoryObj } from "@storybook/react-vite";
import React, { useState } from "react";
import {
  BuildWizard,
  TextInputStep,
  SelectStep,
  CheckboxStep,
  WizardStep,
} from "@calimero-network/mero-ui";

const meta: Meta<typeof BuildWizard> = {
  title: "Forms/Build Wizard",
  component: BuildWizard,
  parameters: {
    layout: "fullscreen",
  },
  argTypes: {
    steps: {
      control: { type: "object" },
      description: "Array of wizard steps",
    },
    initialData: {
      control: { type: "object" },
      description: "Initial data for the wizard",
    },
    onComplete: {
      action: "completed",
      description: "Callback when wizard is completed",
    },
    onCancel: {
      action: "cancelled",
      description: "Callback when wizard is cancelled",
    },
    onStepChange: {
      action: "step changed",
      description: "Callback when step changes",
    },
    showProgress: {
      control: { type: "boolean" },
      description: "Whether to show progress bar",
    },
    showStepNumbers: {
      control: { type: "boolean" },
      description: "Whether to show step numbers",
    },
    allowSkip: {
      control: { type: "boolean" },
      description: "Whether to allow skipping steps",
    },
  },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof BuildWizard>;

// Sample step components
const ProjectInfoStep = ({ data, updateData }: any) => (
  <div style={{ display: "flex", flexDirection: "column", gap: "24px" }}>
    <TextInputStep
      data={data}
      updateData={updateData}
      label="Project Name"
      placeholder="Enter your project name"
      required
    />
    <TextInputStep
      data={data}
      updateData={updateData}
      label="Description"
      placeholder="Describe your project"
      multiline
      rows={4}
    />
    <SelectStep
      data={data}
      updateData={updateData}
      label="Project Type"
      options={[
        { value: "web", label: "Web Application" },
        { value: "mobile", label: "Mobile App" },
        { value: "desktop", label: "Desktop App" },
        { value: "api", label: "API Service" },
      ]}
      required
    />
  </div>
);

const TechnologyStep = ({ data, updateData }: any) => (
  <div style={{ display: "flex", flexDirection: "column", gap: "24px" }}>
    <CheckboxStep
      data={data}
      updateData={updateData}
      label="Frontend Technologies"
      options={[
        {
          value: "react",
          label: "React",
          description: "A JavaScript library for building user interfaces",
        },
        {
          value: "vue",
          label: "Vue.js",
          description: "A progressive JavaScript framework",
        },
        {
          value: "angular",
          label: "Angular",
          description:
            "A platform for building mobile and desktop web applications",
        },
        {
          value: "svelte",
          label: "Svelte",
          description: "A radical new approach to building user interfaces",
        },
      ]}
    />
    <CheckboxStep
      data={data}
      updateData={updateData}
      label="Backend Technologies"
      options={[
        {
          value: "nodejs",
          label: "Node.js",
          description: "JavaScript runtime built on Chrome's V8 engine",
        },
        {
          value: "python",
          label: "Python",
          description: "A high-level programming language",
        },
        {
          value: "java",
          label: "Java",
          description: "A general-purpose programming language",
        },
        {
          value: "go",
          label: "Go",
          description: "An open source programming language",
        },
      ]}
    />
  </div>
);

const ConfigurationStep = ({ data, updateData }: any) => (
  <div style={{ display: "flex", flexDirection: "column", gap: "24px" }}>
    <SelectStep
      data={data}
      updateData={updateData}
      label="Deployment Environment"
      options={[
        { value: "development", label: "Development" },
        { value: "staging", label: "Staging" },
        { value: "production", label: "Production" },
      ]}
      required
    />
    <TextInputStep
      data={data}
      updateData={updateData}
      label="Database URL"
      placeholder="postgresql://user:password@localhost:5432/dbname"
    />
    <CheckboxStep
      data={data}
      updateData={updateData}
      label="Additional Features"
      options={[
        {
          value: "auth",
          label: "Authentication",
          description: "User authentication and authorization",
        },
        {
          value: "analytics",
          label: "Analytics",
          description: "Usage analytics and monitoring",
        },
        {
          value: "logging",
          label: "Logging",
          description: "Application logging and error tracking",
        },
        {
          value: "testing",
          label: "Testing",
          description: "Unit and integration tests",
        },
      ]}
    />
  </div>
);

const ReviewStep = ({ data }: any) => (
  <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
    <h3 style={{ color: "var(--color-neutral-100)", margin: "0 0 16px 0" }}>
      Review Your Configuration
    </h3>

    <div
      style={{
        background: "var(--color-background-secondary)",
        padding: "16px",
        borderRadius: "8px",
        border: "1px solid var(--color-neutral-700)",
      }}
    >
      <h4 style={{ color: "var(--color-neutral-200)", margin: "0 0 8px 0" }}>
        Project Information
      </h4>
      <p style={{ color: "var(--color-neutral-300)", margin: "0 0 4px 0" }}>
        <strong>Name:</strong> {data.project_name || "Not specified"}
      </p>
      <p style={{ color: "var(--color-neutral-300)", margin: "0 0 4px 0" }}>
        <strong>Type:</strong> {data.project_type || "Not specified"}
      </p>
      <p style={{ color: "var(--color-neutral-300)", margin: "0" }}>
        <strong>Description:</strong> {data.description || "Not specified"}
      </p>
    </div>

    <div
      style={{
        background: "var(--color-background-secondary)",
        padding: "16px",
        borderRadius: "8px",
        border: "1px solid var(--color-neutral-700)",
      }}
    >
      <h4 style={{ color: "var(--color-neutral-200)", margin: "0 0 8px 0" }}>
        Technologies
      </h4>
      <p style={{ color: "var(--color-neutral-300)", margin: "0 0 4px 0" }}>
        <strong>Frontend:</strong>{" "}
        {data.frontend_technologies?.join(", ") || "None selected"}
      </p>
      <p style={{ color: "var(--color-neutral-300)", margin: "0" }}>
        <strong>Backend:</strong>{" "}
        {data.backend_technologies?.join(", ") || "None selected"}
      </p>
    </div>

    <div
      style={{
        background: "var(--color-background-secondary)",
        padding: "16px",
        borderRadius: "8px",
        border: "1px solid var(--color-neutral-700)",
      }}
    >
      <h4 style={{ color: "var(--color-neutral-200)", margin: "0 0 8px 0" }}>
        Configuration
      </h4>
      <p style={{ color: "var(--color-neutral-300)", margin: "0 0 4px 0" }}>
        <strong>Environment:</strong>{" "}
        {data.deployment_environment || "Not specified"}
      </p>
      <p style={{ color: "var(--color-neutral-300)", margin: "0 0 4px 0" }}>
        <strong>Database:</strong> {data.database_url || "Not specified"}
      </p>
      <p style={{ color: "var(--color-neutral-300)", margin: "0" }}>
        <strong>Features:</strong>{" "}
        {data.additional_features?.join(", ") || "None selected"}
      </p>
    </div>
  </div>
);

const sampleSteps: WizardStep[] = [
  {
    id: "project-info",
    title: "Project Information",
    description: "Tell us about your project",
    icon: "folder",
    component: ProjectInfoStep,
    validation: (data) => {
      return !!(data.project_name && data.project_type);
    },
  },
  {
    id: "technologies",
    title: "Technologies",
    description: "Select your preferred technologies",
    icon: "code",
    component: TechnologyStep,
  },
  {
    id: "configuration",
    title: "Configuration",
    description: "Configure your deployment settings",
    icon: "settings",
    component: ConfigurationStep,
    validation: (data) => {
      return !!data.deployment_environment;
    },
  },
  {
    id: "review",
    title: "Review",
    description: "Review your configuration before proceeding",
    icon: "check-circle",
    component: ReviewStep,
  },
];

export const Default: Story = {
  args: {
    steps: sampleSteps,
    initialData: {},
    showProgress: true,
    showStepNumbers: true,
    allowSkip: false,
  },
};

export const WithSkipOption: Story = {
  args: {
    steps: sampleSteps,
    initialData: {},
    showProgress: true,
    showStepNumbers: true,
    allowSkip: true,
  },
};

export const Minimal: Story = {
  args: {
    steps: sampleSteps,
    initialData: {},
    showProgress: false,
    showStepNumbers: false,
    allowSkip: false,
  },
};

export const WithInitialData: Story = {
  args: {
    steps: sampleSteps,
    initialData: {
      project_name: "My Awesome Project",
      project_type: "web",
      description: "A revolutionary web application",
      frontend_technologies: ["react"],
      backend_technologies: ["nodejs"],
      deployment_environment: "production",
      additional_features: ["auth", "analytics"],
    },
    showProgress: true,
    showStepNumbers: true,
    allowSkip: false,
  },
};

export const SimpleWizard: Story = {
  args: {
    steps: [
      {
        id: "name",
        title: "Your Name",
        description: "What should we call you?",
        icon: "user",
        component: ({ data, updateData }) => (
          <TextInputStep
            data={data}
            updateData={updateData}
            label="Full Name"
            placeholder="Enter your full name"
            required
          />
        ),
        validation: (data) => !!data.full_name,
      },
      {
        id: "email",
        title: "Contact Information",
        description: "How can we reach you?",
        icon: "mail",
        component: ({ data, updateData }) => (
          <div
            style={{ display: "flex", flexDirection: "column", gap: "16px" }}
          >
            <TextInputStep
              data={data}
              updateData={updateData}
              label="Email Address"
              placeholder="your@email.com"
              required
            />
            <TextInputStep
              data={data}
              updateData={updateData}
              label="Phone Number"
              placeholder="+1 (555) 123-4567"
            />
          </div>
        ),
        validation: (data) => !!data.email_address,
      },
      {
        id: "preferences",
        title: "Preferences",
        description: "Tell us about your preferences",
        icon: "heart",
        component: ({ data, updateData }) => (
          <CheckboxStep
            data={data}
            updateData={updateData}
            label="Interests"
            options={[
              { value: "tech", label: "Technology" },
              { value: "design", label: "Design" },
              { value: "business", label: "Business" },
              { value: "marketing", label: "Marketing" },
            ]}
          />
        ),
      },
    ],
    initialData: {},
    showProgress: true,
    showStepNumbers: true,
    allowSkip: true,
  },
};

export const InteractiveExample: Story = {
  render: () => {
    const [wizardData, setWizardData] = useState({});
    const [isCompleted, setIsCompleted] = useState(false);

    const handleComplete = (data: any) => {
      setWizardData(data);
      setIsCompleted(true);
    };

    if (isCompleted) {
      return (
        <div
          style={{
            padding: "24px",
            background: "var(--color-background-primary)",
            borderRadius: "12px",
            border: "1px solid var(--color-neutral-700)",
            textAlign: "center",
          }}
        >
          <h2
            style={{ color: "var(--color-neutral-100)", marginBottom: "16px" }}
          >
            🎉 Wizard Completed!
          </h2>
          <p
            style={{ color: "var(--color-neutral-300)", marginBottom: "24px" }}
          >
            Your configuration has been saved successfully.
          </p>
          <pre
            style={{
              background: "var(--color-background-secondary)",
              padding: "16px",
              borderRadius: "8px",
              color: "var(--color-neutral-200)",
              textAlign: "left",
              overflow: "auto",
            }}
          >
            {JSON.stringify(wizardData, null, 2)}
          </pre>
          <button
            onClick={() => {
              setWizardData({});
              setIsCompleted(false);
            }}
            style={{
              marginTop: "16px",
              padding: "8px 16px",
              background: "var(--color-brand-600)",
              color: "white",
              border: "none",
              borderRadius: "6px",
              cursor: "pointer",
            }}
          >
            Start Over
          </button>
        </div>
      );
    }

    return (
      <BuildWizard
        steps={sampleSteps}
        initialData={{}}
        onComplete={handleComplete}
        onCancel={() => setIsCompleted(true)}
        showProgress={true}
        showStepNumbers={true}
        allowSkip={true}
      />
    );
  },
};
