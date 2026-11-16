import type { Meta, StoryObj } from "@storybook/react-vite";
import { FilePreview } from "@calimero-network/mero-ui";

// Mock files for stories
const createMockFile = (name: string, size: number, type: string) => {
  const file = new File([""], name, { type });
  Object.defineProperty(file, "size", { value: size });
  return file;
};

const mockFiles = {
  image: createMockFile("photo.jpg", 1024 * 1024 * 2.5, "image/jpeg"),
  pdf: createMockFile("document.pdf", 1024 * 1024 * 1.2, "application/pdf"),
  video: createMockFile("video.mp4", 1024 * 1024 * 50, "video/mp4"),
  audio: createMockFile("music.mp3", 1024 * 1024 * 4.8, "audio/mpeg"),
  word: createMockFile(
    "report.docx",
    1024 * 1024 * 0.8,
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  ),
  excel: createMockFile(
    "data.xlsx",
    1024 * 1024 * 1.5,
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  ),
  zip: createMockFile("archive.zip", 1024 * 1024 * 10, "application/zip"),
  text: createMockFile("notes.txt", 1024 * 0.5, "text/plain"),
};

const meta: Meta<typeof FilePreview> = {
  title: "Utility/File Preview",
  component: FilePreview,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    file: {
      control: false,
      description: "File object to preview",
    },
    showSize: {
      control: { type: "boolean" },
      description: "Whether to show file size",
    },
    variant: {
      control: { type: "select" },
      options: ["compact", "detailed"],
      description: "Display variant",
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
      description: "Size variant",
    },
  },
};

export default meta;
type Story = StoryObj<typeof FilePreview>;

export const Default: Story = {
  args: {
    file: mockFiles.image,
    showSize: true,
    variant: "compact",
    size: "md",
  },
};

export const WithRemove: Story = {
  args: {
    file: mockFiles.pdf,
    showSize: true,
    variant: "compact",
    size: "md",
    onRemove: () => console.log("Remove file"),
  },
};

export const Detailed: Story = {
  args: {
    file: mockFiles.video,
    showSize: true,
    variant: "detailed",
    size: "md",
  },
};

export const WithoutSize: Story = {
  args: {
    file: mockFiles.audio,
    showSize: false,
    variant: "compact",
    size: "md",
  },
};

export const AllSizes: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
      <FilePreview file={mockFiles.image} size="sm" />
      <FilePreview file={mockFiles.image} size="md" />
      <FilePreview file={mockFiles.image} size="lg" />
    </div>
  ),
};

export const FileTypes: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
      <FilePreview file={mockFiles.image} />
      <FilePreview file={mockFiles.pdf} />
      <FilePreview file={mockFiles.video} />
      <FilePreview file={mockFiles.audio} />
      <FilePreview file={mockFiles.word} />
      <FilePreview file={mockFiles.excel} />
      <FilePreview file={mockFiles.zip} />
      <FilePreview file={mockFiles.text} />
    </div>
  ),
};

export const WithRemoveAll: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
      <FilePreview
        file={mockFiles.image}
        onRemove={() => console.log("Remove image")}
      />
      <FilePreview
        file={mockFiles.pdf}
        onRemove={() => console.log("Remove pdf")}
      />
      <FilePreview
        file={mockFiles.video}
        onRemove={() => console.log("Remove video")}
      />
      <FilePreview
        file={mockFiles.audio}
        onRemove={() => console.log("Remove audio")}
      />
    </div>
  ),
};

export const CompactVsDetailed: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <div>
        <h4>Compact Variant</h4>
        <FilePreview file={mockFiles.pdf} variant="compact" />
      </div>
      <div>
        <h4>Detailed Variant</h4>
        <FilePreview file={mockFiles.pdf} variant="detailed" />
      </div>
    </div>
  ),
};
