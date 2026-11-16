import type { Meta, StoryObj } from "@storybook/react-vite";
import { DataTable } from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof DataTable> = {
  title: "Data Display/Data Table",
  component: DataTable,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    data: {
      control: { type: "object" },
      description: "Array of data objects",
    },
    columns: {
      control: { type: "object" },
      description: "Array of column configurations",
    },
    loading: {
      control: { type: "boolean" },
      description: "Show loading state",
    },
    searchable: {
      control: { type: "boolean" },
      description: "Enable search functionality",
    },
    sortable: {
      control: { type: "boolean" },
      description: "Enable sorting functionality",
    },
    selectable: {
      control: { type: "boolean" },
      description: "Enable row selection",
    },
    size: {
      control: { type: "select" },
      options: ["small", "medium", "large"],
      description: "Table size",
    },
    bordered: {
      control: { type: "boolean" },
      description: "Show table borders",
    },
    striped: {
      control: { type: "boolean" },
      description: "Alternating row colors",
    },
    hoverable: {
      control: { type: "boolean" },
      description: "Row hover effects",
    },
  },
};

export default meta;
type Story = StoryObj<typeof DataTable>;

// Sample data
const sampleData = [
  {
    id: 1,
    name: "John Doe",
    email: "john@example.com",
    role: "Admin",
    status: "Active",
    lastLogin: "2024-01-15",
    avatar: "JD",
  },
  {
    id: 2,
    name: "Jane Smith",
    email: "jane@example.com",
    role: "User",
    status: "Inactive",
    lastLogin: "2024-01-10",
    avatar: "JS",
  },
  {
    id: 3,
    name: "Bob Johnson",
    email: "bob@example.com",
    role: "Moderator",
    status: "Active",
    lastLogin: "2024-01-14",
    avatar: "BJ",
  },
  {
    id: 4,
    name: "Alice Brown",
    email: "alice@example.com",
    role: "User",
    status: "Pending",
    lastLogin: "2024-01-12",
    avatar: "AB",
  },
  {
    id: 5,
    name: "Charlie Wilson",
    email: "charlie@example.com",
    role: "Admin",
    status: "Active",
    lastLogin: "2024-01-16",
    avatar: "CW",
  },
];

const columns = [
  {
    key: "name",
    title: "Name",
    dataIndex: "name" as keyof (typeof sampleData)[0],
    sortable: true,
    filterable: true,
    render: (value: string, record: (typeof sampleData)[0]) => (
      <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
        <div
          style={{
            width: "32px",
            height: "32px",
            borderRadius: "50%",
            background: "var(--color-brand-600)",
            color: "#000000",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: "12px",
            fontWeight: "600",
          }}
        >
          {record.avatar}
        </div>
        <span>{value}</span>
      </div>
    ),
  },
  {
    key: "email",
    title: "Email",
    dataIndex: "email" as keyof (typeof sampleData)[0],
    sortable: true,
    filterable: true,
  },
  {
    key: "role",
    title: "Role",
    dataIndex: "role" as keyof (typeof sampleData)[0],
    sortable: true,
    filterable: true,
    render: (value: string) => (
      <span
        style={{
          padding: "4px 8px",
          borderRadius: "4px",
          fontSize: "12px",
          fontWeight: "500",
          background:
            value === "Admin"
              ? "var(--color-brand-600)20"
              : "var(--color-neutral-600)20",
          color: value === "Admin" ? "var(--color-brand-600)" : "#FFFFFF",
        }}
      >
        {value}
      </span>
    ),
  },
  {
    key: "status",
    title: "Status",
    dataIndex: "status" as keyof (typeof sampleData)[0],
    sortable: true,
    filterable: true,
    render: (value: string) => {
      const statusColors = {
        Active: "var(--color-semantic-success)",
        Inactive: "var(--color-semantic-error)",
        Pending: "var(--color-semantic-warning)",
      };

      return (
        <span
          style={{
            padding: "4px 8px",
            borderRadius: "4px",
            fontSize: "12px",
            fontWeight: "500",
            background: `${statusColors[value as keyof typeof statusColors]}20`,
            color: statusColors[value as keyof typeof statusColors],
          }}
        >
          {value}
        </span>
      );
    },
  },
  {
    key: "lastLogin",
    title: "Last Login",
    dataIndex: "lastLogin" as keyof (typeof sampleData)[0],
    sortable: true,
    align: "right" as const,
  },
  {
    key: "actions",
    title: "Actions",
    render: (value: any, record: (typeof sampleData)[0]) => (
      <div style={{ display: "flex", gap: "8px" }}>
        <button
          style={{
            background: "transparent",
            border: "none",
            color: "var(--color-brand-600)",
            cursor: "pointer",
            fontSize: "12px",
            padding: "4px 8px",
            borderRadius: "4px",
          }}
        >
          Edit
        </button>
        <button
          style={{
            background: "transparent",
            border: "none",
            color: "var(--color-semantic-error)",
            cursor: "pointer",
            fontSize: "12px",
            padding: "4px 8px",
            borderRadius: "4px",
          }}
        >
          Delete
        </button>
      </div>
    ),
  },
];

export const Default: Story = {
  args: {
    data: sampleData,
    columns,
    searchable: true,
    sortable: true,
    selectable: true,
    bordered: true,
    striped: true,
    hoverable: true,
  },
};

export const Loading: Story = {
  args: {
    data: [],
    columns,
    loading: true,
    searchable: true,
    sortable: true,
  },
};

export const NoData: Story = {
  args: {
    data: [],
    columns,
    searchable: true,
    sortable: true,
  },
};

export const Small: Story = {
  args: {
    data: sampleData,
    columns,
    size: "small",
    searchable: true,
    sortable: true,
  },
};

export const Large: Story = {
  args: {
    data: sampleData,
    columns,
    size: "large",
    searchable: true,
    sortable: true,
  },
};

export const NoBorders: Story = {
  args: {
    data: sampleData,
    columns,
    bordered: false,
    striped: false,
    hoverable: true,
  },
};

export const WithPagination: Story = {
  args: {
    data: sampleData,
    columns,
    pagination: {
      pageSize: 3,
      showSizeChanger: true,
      showQuickJumper: true,
      showTotal: (total, range) =>
        `Showing ${range[0]}-${range[1]} of ${total} users`,
    },
    searchable: true,
    sortable: true,
  },
};

export const Interactive: Story = {
  render: () => {
    const [selectedRows, setSelectedRows] = useState<string[]>([]);
    const [data, setData] = useState(sampleData);

    const handleSelectionChange = (
      selectedKeys: string[],
      selectedRows: any[],
    ) => {
      setSelectedRows(selectedKeys);
      console.log("Selected rows:", selectedKeys, selectedRows);
    };

    const handleRowClick = (record: any, index: number) => {
      console.log("Row clicked:", record, index);
    };

    const handleSort = (column: string, direction: "asc" | "desc") => {
      console.log("Sort:", column, direction);
    };

    const handleFilter = (filters: Record<string, any>) => {
      console.log("Filter:", filters);
    };

    return (
      <div>
        <div style={{ marginBottom: "16px", color: "#FFFFFF" }}>
          Selected rows: {selectedRows.length} ({selectedRows.join(", ")})
        </div>
        <DataTable
          data={data}
          columns={columns}
          searchable={true}
          sortable={true}
          selectable={true}
          selectedRowKeys={selectedRows}
          onSelectionChange={handleSelectionChange}
          onRowClick={handleRowClick}
          onSort={handleSort}
          onFilter={handleFilter}
          pagination={{
            pageSize: 5,
            showSizeChanger: true,
            showTotal: (total, range) =>
              `Showing ${range[0]}-${range[1]} of ${total} users`,
          }}
        />
      </div>
    );
  },
};
