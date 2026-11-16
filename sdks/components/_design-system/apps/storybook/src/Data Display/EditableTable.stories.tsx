import type { Meta, StoryObj } from "@storybook/react-vite";
import { EditableTable, TableColumn } from "@calimero-network/mero-ui";

interface User {
  id: string;
  name: string;
  email: string;
  age: number;
  role: string;
  status: string;
  bio: string;
  joinDate: string;
}

const meta: Meta<typeof EditableTable> = {
  title: "Data Display/EditableTable",
  component: EditableTable,
  parameters: {
    layout: "padded",
    docs: {
      description: {
        component:
          "A powerful table component with inline editing capabilities. Supports various input types, validation, and real-time data updates.",
      },
    },
  },
  argTypes: {
    data: {
      description: "Array of data objects to display in the table",
      control: "object",
    },
    columns: {
      description:
        "Column configuration defining how each field should be displayed and edited",
      control: "object",
    },
    onDataChange: {
      description: "Callback fired when data is modified",
      action: "data changed",
    },
    onRowAdd: {
      description: "Callback fired when a new row is added",
      action: "row added",
    },
    onRowDelete: {
      description: "Callback fired when a row is deleted",
      action: "row deleted",
    },
    onRowUpdate: {
      description: "Callback fired when a row is updated",
      action: "row updated",
    },
    disabled: {
      description: "Disable all editing functionality",
      control: "boolean",
    },
    loading: {
      description: "Show loading state",
      control: "boolean",
    },
    showAddButton: {
      description: "Show the add row button",
      control: "boolean",
    },
    showDeleteButton: {
      description: "Show delete buttons for each row",
      control: "boolean",
    },
    zebra: {
      description: "Apply zebra striping to rows",
      control: "boolean",
    },
    compact: {
      description: "Use compact spacing",
      control: "boolean",
    },
    stickyHeader: {
      description: "Make header sticky",
      control: "boolean",
    },
  },
};

export default meta;
type Story = StoryObj<typeof EditableTable>;

// Sample data
const sampleUsers: User[] = [
  {
    id: "1",
    name: "John Doe",
    email: "john.doe@example.com",
    age: 28,
    role: "admin",
    status: "active",
    bio: "Software engineer with 5 years of experience in React and TypeScript.",
    joinDate: "2023-01-15",
  },
  {
    id: "2",
    name: "Jane Smith",
    email: "jane.smith@example.com",
    age: 32,
    role: "user",
    status: "active",
    bio: "Product manager passionate about user experience and data-driven decisions.",
    joinDate: "2023-03-22",
  },
  {
    id: "3",
    name: "Bob Johnson",
    email: "bob.johnson@example.com",
    age: 45,
    role: "moderator",
    status: "inactive",
    bio: "Senior developer specializing in backend systems and database optimization.",
    joinDate: "2022-11-08",
  },
];

// Column definitions
const userColumns: TableColumn<User>[] = [
  {
    key: "name",
    title: "Name",
    editable: true,
    type: "text",
    required: true,
    placeholder: "Enter full name",
    validate: (value) => {
      if (!value || value.trim().length < 2) {
        return "Name must be at least 2 characters";
      }
      return null;
    },
  },
  {
    key: "email",
    title: "Email",
    editable: true,
    type: "email",
    required: true,
    placeholder: "Enter email address",
    validate: (value) => {
      const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
      if (!value || !emailRegex.test(value)) {
        return "Please enter a valid email address";
      }
      return null;
    },
  },
  {
    key: "age",
    title: "Age",
    editable: true,
    type: "number",
    required: true,
    min: 18,
    max: 100,
    validate: (value) => {
      if (!value || value < 18 || value > 100) {
        return "Age must be between 18 and 100";
      }
      return null;
    },
  },
  {
    key: "role",
    title: "Role",
    editable: true,
    type: "select",
    options: [
      { value: "admin", label: "Administrator" },
      { value: "moderator", label: "Moderator" },
      { value: "user", label: "User" },
    ],
  },
  {
    key: "status",
    title: "Status",
    editable: true,
    type: "select",
    options: [
      { value: "active", label: "Active" },
      { value: "inactive", label: "Inactive" },
      { value: "pending", label: "Pending" },
    ],
  },
  {
    key: "bio",
    title: "Bio",
    editable: true,
    type: "textarea",
    rows: 2,
    placeholder: "Enter a brief bio",
  },
  {
    key: "joinDate",
    title: "Join Date",
    editable: true,
    type: "date",
  },
];

// Basic editable table
export const Default: Story = {
  args: {
    data: sampleUsers,
    columns: userColumns,
  },
};

// Compact version
export const Compact: Story = {
  args: {
    data: sampleUsers,
    columns: userColumns,
    compact: true,
  },
};

// With zebra striping
export const ZebraStriped: Story = {
  args: {
    data: sampleUsers,
    columns: userColumns,
    zebra: true,
  },
};

// With sticky header
export const StickyHeader: Story = {
  args: {
    data: sampleUsers,
    columns: userColumns,
    stickyHeader: true,
  },
  parameters: {
    docs: {
      description: {
        story:
          "Table with sticky header that remains visible when scrolling through long data sets.",
      },
    },
  },
};

// Disabled state
export const Disabled: Story = {
  args: {
    data: sampleUsers,
    columns: userColumns,
    disabled: true,
  },
  parameters: {
    docs: {
      description: {
        story: "Disabled table where editing is not allowed.",
      },
    },
  },
};

// Loading state
export const Loading: Story = {
  args: {
    data: [],
    columns: userColumns,
    loading: true,
  },
  parameters: {
    docs: {
      description: {
        story: "Table in loading state with no data.",
      },
    },
  },
};

// Without add/delete buttons
export const ReadOnly: Story = {
  args: {
    data: sampleUsers,
    columns: userColumns,
    showAddButton: false,
    showDeleteButton: false,
  },
  parameters: {
    docs: {
      description: {
        story: "Read-only table without add or delete functionality.",
      },
    },
  },
};

// Empty state
export const Empty: Story = {
  args: {
    data: [],
    columns: userColumns,
    emptyStateText: 'No users found. Click "Add Row" to create a new user.',
  },
  parameters: {
    docs: {
      description: {
        story: "Empty table with custom empty state message.",
      },
    },
  },
};

// Different input types showcase
const showcaseData = [
  {
    id: "1",
    textField: "Sample text",
    numberField: 42,
    selectField: "option2",
    textareaField:
      "This is a longer text that can span multiple lines and provides more space for detailed information.",
    dateField: "2024-01-15",
    emailField: "test@example.com",
    urlField: "https://example.com",
  },
];

const showcaseColumns: TableColumn<(typeof showcaseData)[0]>[] = [
  {
    key: "textField",
    title: "Text Input",
    editable: true,
    type: "text",
    placeholder: "Enter text",
  },
  {
    key: "numberField",
    title: "Number Input",
    editable: true,
    type: "number",
    min: 0,
    max: 100,
    step: 1,
  },
  {
    key: "selectField",
    title: "Select Dropdown",
    editable: true,
    type: "select",
    options: [
      { value: "option1", label: "Option 1" },
      { value: "option2", label: "Option 2" },
      { value: "option3", label: "Option 3" },
    ],
  },
  {
    key: "textareaField",
    title: "Textarea",
    editable: true,
    type: "textarea",
    rows: 3,
  },
  {
    key: "dateField",
    title: "Date Picker",
    editable: true,
    type: "date",
  },
  {
    key: "emailField",
    title: "Email Input",
    editable: true,
    type: "email",
  },
  {
    key: "urlField",
    title: "URL Input",
    editable: true,
    type: "url",
  },
];

export const InputTypesShowcase: Story = {
  args: {
    data: showcaseData,
    columns: showcaseColumns,
  },
  parameters: {
    docs: {
      description: {
        story:
          "Showcase of different input types available in the EditableTable component.",
      },
    },
  },
};

// Custom validation example
const validationData = [
  {
    id: "1",
    username: "user1",
    email: "user1@example.com",
    score: 85,
  },
  {
    id: "2",
    username: "a", // Invalid: too short
    email: "invalid-email", // Invalid: not a valid email
    score: 150, // Invalid: too high
  },
];

const validationColumns: TableColumn<(typeof validationData)[0]>[] = [
  {
    key: "username",
    title: "Username",
    editable: true,
    type: "text",
    required: true,
    validate: (value) => {
      if (!value || value.length < 3) {
        return "Username must be at least 3 characters";
      }
      if (!/^[a-zA-Z0-9_]+$/.test(value)) {
        return "Username can only contain letters, numbers, and underscores";
      }
      return null;
    },
  },
  {
    key: "email",
    title: "Email",
    editable: true,
    type: "email",
    required: true,
    validate: (value) => {
      const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
      if (!value || !emailRegex.test(value)) {
        return "Please enter a valid email address";
      }
      return null;
    },
  },
  {
    key: "score",
    title: "Score",
    editable: true,
    type: "number",
    min: 0,
    max: 100,
    validate: (value) => {
      if (value < 0 || value > 100) {
        return "Score must be between 0 and 100";
      }
      return null;
    },
  },
];

export const ValidationExample: Story = {
  args: {
    data: validationData,
    columns: validationColumns,
  },
  parameters: {
    docs: {
      description: {
        story:
          "Example showing custom validation with error messages for different field types.",
      },
    },
  },
};
