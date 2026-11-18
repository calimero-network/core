import type { Meta, StoryObj } from "@storybook/react-vite";
import { FileUpload } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof FileUpload> = {
  title: "Forms/File Upload",
  component: FileUpload,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    accept: {
      control: { type: "text" },
      description: "Accepted file types",
    },
    multiple: {
      control: { type: "boolean" },
      description: "Allow multiple files",
    },
    maxSize: {
      control: { type: "number" },
      description: "Maximum file size in bytes",
    },
    maxFiles: {
      control: { type: "number" },
      description: "Maximum number of files",
    },
    disabled: {
      control: { type: "boolean" },
      description: "Disable the upload",
    },
    variant: {
      control: { type: "select" },
      options: ["default", "compact", "minimal"],
      description: "Upload area variant",
    },
    showPreview: {
      control: { type: "boolean" },
      description: "Show file preview",
    },
    showProgress: {
      control: { type: "boolean" },
      description: "Show upload progress",
    },
    placeholder: {
      control: { type: "text" },
      description: "Upload area placeholder",
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
      description: "Upload label",
    },
    required: {
      control: { type: "boolean" },
      description: "Required field",
    },
  },
};

export default meta;
type Story = StoryObj<typeof FileUpload>;

export const Default: Story = {
  args: {
    placeholder: "Click to upload or drag and drop",
  },
};

export const WithLabel: Story = {
  args: {
    label: "Upload Documents",
    placeholder: "Select files to upload",
    required: true,
  },
};

export const MultipleFiles: Story = {
  args: {
    label: "Upload Multiple Files",
    placeholder: "Select multiple files",
    multiple: true,
    maxFiles: 5,
  },
};

export const ImageOnly: Story = {
  args: {
    label: "Upload Images",
    placeholder: "Select image files",
    accept: "image/*",
    multiple: true,
    maxFiles: 10,
  },
};

export const DocumentOnly: Story = {
  args: {
    label: "Upload Documents",
    placeholder: "Select document files",
    accept: ".pdf,.doc,.docx,.txt",
    multiple: true,
  },
};

export const WithSizeLimit: Story = {
  args: {
    label: "Upload with Size Limit",
    placeholder: "Max 5MB per file",
    maxSize: 5 * 1024 * 1024, // 5MB
    multiple: true,
  },
};

export const Compact: Story = {
  args: {
    label: "Compact Upload",
    variant: "compact",
    placeholder: "Upload files",
  },
};

export const Minimal: Story = {
  args: {
    label: "Minimal Upload",
    variant: "minimal",
    placeholder: "Upload",
  },
};

export const WithProgress: Story = {
  args: {
    label: "Upload with Progress",
    placeholder: "Select files to upload",
    showProgress: true,
    multiple: true,
  },
};

export const Disabled: Story = {
  args: {
    label: "Disabled Upload",
    placeholder: "This is disabled",
    disabled: true,
  },
};

export const Error: Story = {
  args: {
    label: "Upload with Error",
    placeholder: "Select files",
    error: true,
    errorMessage: "Please select valid files",
  },
};

export const Controlled: Story = {
  render: () => {
    const [files, setFiles] = useState<File[]>([]);

    const handleUpload = (uploadedFiles: File[]) => {
      setFiles(uploadedFiles);
      console.log("Files uploaded:", uploadedFiles);
    };

    const handleRemove = (file: File) => {
      setFiles((prev) => prev.filter((f) => f !== file));
      console.log("File removed:", file);
    };

    return (
      <div>
        <FileUpload
          label="Controlled File Upload"
          files={files}
          onUpload={handleUpload}
          onRemove={handleRemove}
          placeholder="Select files to upload"
          multiple={true}
          showPreview={true}
        />
        <div style={{ marginTop: "16px", color: "#FFFFFF" }}>
          <h4>Uploaded Files ({files.length}):</h4>
          <ul>
            {files.map((file, index) => (
              <li key={index}>
                {file.name} ({(file.size / 1024).toFixed(2)} KB)
              </li>
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
      avatar: [] as File[],
      documents: [] as File[],
      images: [] as File[],
    });

    const handleSubmit = (e: React.FormEvent) => {
      e.preventDefault();
      alert(
        `Form submitted with:\nAvatar: ${formData.avatar.length} file(s)\nDocuments: ${formData.documents.length} file(s)\nImages: ${formData.images.length} file(s)`,
      );
    };

    return (
      <form onSubmit={handleSubmit} style={{ maxWidth: "500px" }}>
        <h3 style={{ color: "#FFFFFF", marginBottom: "20px" }}>
          File Upload Form
        </h3>

        <div style={{ marginBottom: "20px" }}>
          <FileUpload
            label="Profile Picture"
            files={formData.avatar}
            onUpload={(files) =>
              setFormData((prev) => ({ ...prev, avatar: files }))
            }
            onRemove={(file) =>
              setFormData((prev) => ({
                ...prev,
                avatar: prev.avatar.filter((f) => f !== file),
              }))
            }
            placeholder="Select profile picture"
            accept="image/*"
            maxFiles={1}
            maxSize={2 * 1024 * 1024} // 2MB
            required
          />
        </div>

        <div style={{ marginBottom: "20px" }}>
          <FileUpload
            label="Documents"
            files={formData.documents}
            onUpload={(files) =>
              setFormData((prev) => ({ ...prev, documents: files }))
            }
            onRemove={(file) =>
              setFormData((prev) => ({
                ...prev,
                documents: prev.documents.filter((f) => f !== file),
              }))
            }
            placeholder="Select document files"
            accept=".pdf,.doc,.docx,.txt"
            multiple={true}
            maxFiles={5}
          />
        </div>

        <div style={{ marginBottom: "20px" }}>
          <FileUpload
            label="Gallery Images"
            files={formData.images}
            onUpload={(files) =>
              setFormData((prev) => ({ ...prev, images: files }))
            }
            onRemove={(file) =>
              setFormData((prev) => ({
                ...prev,
                images: prev.images.filter((f) => f !== file),
              }))
            }
            placeholder="Select gallery images"
            accept="image/*"
            multiple={true}
            maxFiles={10}
            showPreview={true}
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
          Submit Files
        </button>
      </form>
    );
  },
};
