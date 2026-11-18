import type { Meta, StoryObj } from "@storybook/react-vite";
import { TagInput } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof TagInput> = {
  title: "Forms/Tag Input",
  component: TagInput,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    value: {
      control: { type: "object" },
      description: "Array of tag objects",
    },
    suggestions: {
      control: { type: "object" },
      description: "Array of suggestion strings",
    },
    placeholder: {
      control: { type: "text" },
      description: "Input placeholder",
    },
    disabled: {
      control: { type: "boolean" },
      description: "Disable the input",
    },
    maxTags: {
      control: { type: "number" },
      description: "Maximum number of tags",
    },
    allowDuplicates: {
      control: { type: "boolean" },
      description: "Allow duplicate tags",
    },
    size: {
      control: { type: "select" },
      options: ["small", "medium", "large"],
      description: "Input size",
    },
    variant: {
      control: { type: "select" },
      options: ["default", "outlined", "filled"],
      description: "Input variant",
    },
    showSuggestions: {
      control: { type: "boolean" },
      description: "Show suggestions dropdown",
    },
    maxSuggestions: {
      control: { type: "number" },
      description: "Maximum number of suggestions",
    },
    createTagOnEnter: {
      control: { type: "boolean" },
      description: "Create tag on Enter key",
    },
    createTagOnComma: {
      control: { type: "boolean" },
      description: "Create tag on comma key",
    },
    createTagOnSpace: {
      control: { type: "boolean" },
      description: "Create tag on space key",
    },
    separator: {
      control: { type: "text" },
      description: "Tag separator",
    },
    caseSensitive: {
      control: { type: "boolean" },
      description: "Case sensitive duplicate checking",
    },
  },
};

export default meta;
type Story = StoryObj<typeof TagInput>;

// Sample suggestions
const sampleSuggestions = [
  "React",
  "Vue.js",
  "Angular",
  "TypeScript",
  "JavaScript",
  "Python",
  "Node.js",
  "Express",
  "MongoDB",
  "PostgreSQL",
  "Redis",
  "Docker",
  "Kubernetes",
  "AWS",
  "Azure",
  "Git",
  "GitHub",
  "GitLab",
  "VS Code",
  "WebStorm",
];

export const Default: Story = {
  args: {
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
  },
};

export const WithLabel: Story = {
  args: {
    label: "Technologies",
    suggestions: sampleSuggestions,
    placeholder: "Add technologies...",
    required: true,
  },
};

export const WithMaxTags: Story = {
  args: {
    label: "Skills (Max 5)",
    suggestions: sampleSuggestions,
    placeholder: "Add skills...",
    maxTags: 5,
  },
};

export const AllowDuplicates: Story = {
  args: {
    label: "Tags (Duplicates Allowed)",
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
    allowDuplicates: true,
  },
};

export const Outlined: Story = {
  args: {
    label: "Outlined Tags",
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
    variant: "outlined",
  },
};

export const Filled: Story = {
  args: {
    label: "Filled Tags",
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
    variant: "filled",
  },
};

export const Small: Story = {
  args: {
    label: "Small Tags",
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
    size: "small",
  },
};

export const Large: Story = {
  args: {
    label: "Large Tags",
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
    size: "large",
  },
};

export const Disabled: Story = {
  args: {
    label: "Disabled Tags",
    suggestions: sampleSuggestions,
    placeholder: "This is disabled",
    disabled: true,
  },
};

export const WithoutSuggestions: Story = {
  args: {
    label: "Tags without Suggestions",
    suggestions: [],
    placeholder: "Add tags...",
    showSuggestions: false,
  },
};

export const CreateOnComma: Story = {
  args: {
    label: "Create on Comma",
    suggestions: sampleSuggestions,
    placeholder: "Add tags (press comma to create)...",
    createTagOnComma: true,
    createTagOnEnter: false,
  },
};

export const CreateOnSpace: Story = {
  args: {
    label: "Create on Space",
    suggestions: sampleSuggestions,
    placeholder: "Add tags (press space to create)...",
    createTagOnSpace: true,
    createTagOnEnter: false,
  },
};

export const CaseSensitive: Story = {
  args: {
    label: "Case Sensitive",
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
    caseSensitive: true,
  },
};

export const WithValidation: Story = {
  args: {
    label: "Tags with Validation",
    suggestions: sampleSuggestions,
    placeholder: "Add tags...",
    validateTag: (tag) => {
      if (tag.length < 2) {
        return "Tag must be at least 2 characters long";
      }
      if (tag.length > 20) {
        return "Tag must be less than 20 characters";
      }
      if (!/^[a-zA-Z0-9\s-]+$/.test(tag)) {
        return "Tag can only contain letters, numbers, spaces, and hyphens";
      }
      return true;
    },
  },
};

export const Controlled: Story = {
  render: () => {
    const [tags, setTags] = useState([
      { id: "1", label: "React", value: "react" },
      { id: "2", label: "TypeScript", value: "typescript" },
    ]);

    const handleAdd = (tag: any) => {
      console.log("Tag added:", tag);
    };

    const handleRemove = (tag: any) => {
      console.log("Tag removed:", tag);
    };

    return (
      <div>
        <TagInput
          label="Controlled Tags"
          value={tags}
          onChange={setTags}
          onAdd={handleAdd}
          onRemove={handleRemove}
          suggestions={sampleSuggestions}
          placeholder="Add tags..."
          maxTags={10}
        />
        <div style={{ marginTop: "16px", color: "#FFFFFF" }}>
          <h4>Current Tags ({tags.length}):</h4>
          <ul>
            {tags.map((tag) => (
              <li key={tag.id}>{tag.label}</li>
            ))}
          </ul>
        </div>
      </div>
    );
  },
};

export const FormExample: Story = {
  render: () => {
    const [formData, setFormData] = useState({
      skills: [] as any[],
      interests: [] as any[],
      languages: [] as any[],
    });

    const handleSubmit = (e: React.FormEvent) => {
      e.preventDefault();
      alert(
        `Form submitted with:\nSkills: ${formData.skills.map((t) => t.label).join(", ")}\nInterests: ${formData.interests.map((t) => t.label).join(", ")}\nLanguages: ${formData.languages.map((t) => t.label).join(", ")}`,
      );
    };

    const skillSuggestions = [
      "JavaScript",
      "TypeScript",
      "React",
      "Vue.js",
      "Angular",
      "Node.js",
      "Python",
      "Java",
      "C#",
      "Go",
      "Rust",
      "PHP",
      "Ruby",
      "Swift",
      "Kotlin",
    ];

    const interestSuggestions = [
      "Web Development",
      "Mobile Development",
      "Machine Learning",
      "AI",
      "Data Science",
      "DevOps",
      "Cloud Computing",
      "Cybersecurity",
      "Blockchain",
      "IoT",
      "AR/VR",
      "Game Development",
    ];

    const languageSuggestions = [
      "English",
      "Spanish",
      "French",
      "German",
      "Italian",
      "Portuguese",
      "Chinese",
      "Japanese",
      "Korean",
      "Arabic",
      "Russian",
      "Hindi",
    ];

    return (
      <form onSubmit={handleSubmit} style={{ maxWidth: "500px" }}>
        <h3 style={{ color: "#FFFFFF", marginBottom: "20px" }}>Profile Form</h3>

        <div style={{ marginBottom: "20px" }}>
          <TagInput
            label="Skills"
            value={formData.skills}
            onChange={(skills) => setFormData((prev) => ({ ...prev, skills }))}
            suggestions={skillSuggestions}
            placeholder="Add your skills..."
            maxTags={10}
            required
          />
        </div>

        <div style={{ marginBottom: "20px" }}>
          <TagInput
            label="Interests"
            value={formData.interests}
            onChange={(interests) =>
              setFormData((prev) => ({ ...prev, interests }))
            }
            suggestions={interestSuggestions}
            placeholder="Add your interests..."
            maxTags={8}
          />
        </div>

        <div style={{ marginBottom: "20px" }}>
          <TagInput
            label="Languages"
            value={formData.languages}
            onChange={(languages) =>
              setFormData((prev) => ({ ...prev, languages }))
            }
            suggestions={languageSuggestions}
            placeholder="Add languages you speak..."
            maxTags={5}
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
          Submit Profile
        </button>
      </form>
    );
  },
};

export const ColorCodedTags: Story = {
  render: () => {
    const [tags, setTags] = useState([
      {
        id: "1",
        label: "Frontend",
        value: "frontend",
        color: "var(--color-brand-600)",
      },
      {
        id: "2",
        label: "Backend",
        value: "backend",
        color: "var(--color-semantic-success)",
      },
      {
        id: "3",
        label: "Database",
        value: "database",
        color: "var(--color-semantic-info)",
      },
    ]);

    const suggestions = [
      "Frontend",
      "Backend",
      "Database",
      "DevOps",
      "Mobile",
      "AI/ML",
      "Design",
      "Testing",
      "Security",
      "Cloud",
      "Blockchain",
      "IoT",
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Color Coded Tags
        </h3>
        <TagInput
          label="Categories"
          value={tags}
          onChange={setTags}
          suggestions={suggestions}
          placeholder="Add categories..."
          maxTags={8}
        />
      </div>
    );
  },
};

export const NonRemovableTags: Story = {
  render: () => {
    const [tags, setTags] = useState([
      { id: "1", label: "Required", value: "required", removable: false },
      { id: "2", label: "Optional", value: "optional", removable: true },
      { id: "3", label: "System", value: "system", removable: false },
    ]);

    const suggestions = [
      "Required",
      "Optional",
      "System",
      "Admin",
      "User",
      "Guest",
      "Premium",
      "Free",
      "Beta",
      "Stable",
      "Deprecated",
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Non-Removable Tags
        </h3>
        <TagInput
          label="Tag Types"
          value={tags}
          onChange={setTags}
          suggestions={suggestions}
          placeholder="Add tag types..."
          maxTags={10}
        />
      </div>
    );
  },
};
