import type { Meta, StoryObj } from "@storybook/react-vite";
import { TreeTable } from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";
import { tokens } from "@calimero-network/mero-tokens";
import { useState } from "react";

const meta: Meta<typeof TreeTable> = {
  title: "Data Display/Tree Table",
  component: TreeTable,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    data: {
      control: { type: "object" },
      description: "Array of tree node data",
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
type Story = StoryObj<typeof TreeTable>;

// Sample tree data
const sampleTreeData = [
  {
    id: "1",
    data: {
      name: "Engineering",
      type: "Department",
      employees: 25,
      budget: "$500,000",
      status: "Active",
    },
    children: [
      {
        id: "1-1",
        data: {
          name: "Frontend Team",
          type: "Team",
          employees: 8,
          budget: "$150,000",
          status: "Active",
        },
        children: [
          {
            id: "1-1-1",
            data: {
              name: "John Doe",
              type: "Employee",
              employees: 0,
              budget: "$80,000",
              status: "Active",
            },
          },
          {
            id: "1-1-2",
            data: {
              name: "Jane Smith",
              type: "Employee",
              employees: 0,
              budget: "$75,000",
              status: "Active",
            },
          },
        ],
      },
      {
        id: "1-2",
        data: {
          name: "Backend Team",
          type: "Team",
          employees: 12,
          budget: "$200,000",
          status: "Active",
        },
        children: [
          {
            id: "1-2-1",
            data: {
              name: "Bob Johnson",
              type: "Employee",
              employees: 0,
              budget: "$90,000",
              status: "Active",
            },
          },
          {
            id: "1-2-2",
            data: {
              name: "Alice Brown",
              type: "Employee",
              employees: 0,
              budget: "$85,000",
              status: "Active",
            },
          },
        ],
      },
    ],
  },
  {
    id: "2",
    data: {
      name: "Marketing",
      type: "Department",
      employees: 15,
      budget: "$300,000",
      status: "Active",
    },
    children: [
      {
        id: "2-1",
        data: {
          name: "Digital Marketing",
          type: "Team",
          employees: 8,
          budget: "$150,000",
          status: "Active",
        },
        children: [
          {
            id: "2-1-1",
            data: {
              name: "Charlie Wilson",
              type: "Employee",
              employees: 0,
              budget: "$70,000",
              status: "Active",
            },
          },
        ],
      },
      {
        id: "2-2",
        data: {
          name: "Content Team",
          type: "Team",
          employees: 7,
          budget: "$120,000",
          status: "Active",
        },
      },
    ],
  },
  {
    id: "3",
    data: {
      name: "Sales",
      type: "Department",
      employees: 20,
      budget: "$400,000",
      status: "Active",
    },
    children: [
      {
        id: "3-1",
        data: {
          name: "Enterprise Sales",
          type: "Team",
          employees: 12,
          budget: "$250,000",
          status: "Active",
        },
      },
      {
        id: "3-2",
        data: {
          name: "SMB Sales",
          type: "Team",
          employees: 8,
          budget: "$150,000",
          status: "Active",
        },
      },
    ],
  },
];

const columns = [
  {
    key: "name",
    title: "Name",
    dataIndex: "name" as keyof (typeof sampleTreeData)[0]["data"],
    sortable: true,
    filterable: true,
    render: (value: string, node: any, level: number) => (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "8px",
          paddingLeft: `${level * 8}px`,
        }}
      >
        {node.children && node.children.length > 0 && (
          <Icon
            name="folder"
            size="xs"
            style={{ color: tokens.color.brand[600].value }}
          />
        )}
        {!node.children && (
          <Icon
            name="user"
            size="xs"
            style={{ color: tokens.color.neutral[400].value }}
          />
        )}
        <span>{value}</span>
      </div>
    ),
  },
  {
    key: "type",
    title: "Type",
    dataIndex: "type" as keyof (typeof sampleTreeData)[0]["data"],
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
            value === "Department"
              ? "var(--color-brand-600)20"
              : value === "Team"
                ? "var(--color-neutral-600)20"
                : "var(--color-background-secondary)",
          color:
            value === "Department"
              ? "var(--color-brand-600)"
              : value === "Team"
                ? "#FFFFFF"
                : "var(--color-neutral-400)",
        }}
      >
        {value}
      </span>
    ),
  },
  {
    key: "employees",
    title: "Employees",
    dataIndex: "employees" as keyof (typeof sampleTreeData)[0]["data"],
    sortable: true,
    align: "right" as const,
  },
  {
    key: "budget",
    title: "Budget",
    dataIndex: "budget" as keyof (typeof sampleTreeData)[0]["data"],
    sortable: true,
    align: "right" as const,
  },
  {
    key: "status",
    title: "Status",
    dataIndex: "status" as keyof (typeof sampleTreeData)[0]["data"],
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
];

export const Default: Story = {
  args: {
    data: sampleTreeData,
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
    data: sampleTreeData,
    columns,
    size: "small",
    searchable: true,
    sortable: true,
  },
};

export const Large: Story = {
  args: {
    data: sampleTreeData,
    columns,
    size: "large",
    searchable: true,
    sortable: true,
  },
};

export const NoBorders: Story = {
  args: {
    data: sampleTreeData,
    columns,
    bordered: false,
    striped: false,
    hoverable: true,
  },
};

export const WithoutSearch: Story = {
  args: {
    data: sampleTreeData,
    columns,
    searchable: false,
    sortable: true,
    selectable: true,
  },
};

export const WithoutSelection: Story = {
  args: {
    data: sampleTreeData,
    columns,
    searchable: true,
    sortable: true,
    selectable: false,
  },
};

export const Interactive: Story = {
  render: () => {
    const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
    const [data, setData] = useState(sampleTreeData);

    const handleSelectionChange = (
      selectedKeys: string[],
      selectedNodes: any[],
    ) => {
      setSelectedKeys(selectedKeys);
      console.log("Selected nodes:", selectedKeys, selectedNodes);
    };

    const handleNodeClick = (node: any, level: number) => {
      console.log("Node clicked:", node, level);
    };

    const handleNodeToggle = (node: any, expanded: boolean) => {
      console.log("Node toggled:", node, expanded);
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
          Selected nodes: {selectedKeys.length} ({selectedKeys.join(", ")})
        </div>
        <TreeTable
          data={data}
          columns={columns}
          searchable={true}
          sortable={true}
          selectable={true}
          selectedKeys={selectedKeys}
          onSelectionChange={handleSelectionChange}
          onNodeClick={handleNodeClick}
          onNodeToggle={handleNodeToggle}
          onSort={handleSort}
          onFilter={handleFilter}
          showExpandAll={true}
          defaultExpandAll={false}
        />
      </div>
    );
  },
};

export const FileSystemExample: Story = {
  render: () => {
    const fileSystemData = [
      {
        id: "root",
        data: {
          name: "Project Root",
          type: "folder",
          size: "2.5 MB",
          modified: "2024-01-15",
          permissions: "rwxr-xr-x",
        },
        children: [
          {
            id: "src",
            data: {
              name: "src",
              type: "folder",
              size: "1.2 MB",
              modified: "2024-01-15",
              permissions: "rwxr-xr-x",
            },
            children: [
              {
                id: "components",
                data: {
                  name: "components",
                  type: "folder",
                  size: "800 KB",
                  modified: "2024-01-14",
                  permissions: "rwxr-xr-x",
                },
                children: [
                  {
                    id: "Button.tsx",
                    data: {
                      name: "Button.tsx",
                      type: "file",
                      size: "15 KB",
                      modified: "2024-01-14",
                      permissions: "rw-r--r--",
                    },
                  },
                  {
                    id: "Input.tsx",
                    data: {
                      name: "Input.tsx",
                      type: "file",
                      size: "12 KB",
                      modified: "2024-01-14",
                      permissions: "rw-r--r--",
                    },
                  },
                ],
              },
              {
                id: "utils",
                data: {
                  name: "utils",
                  type: "folder",
                  size: "400 KB",
                  modified: "2024-01-13",
                  permissions: "rwxr-xr-x",
                },
                children: [
                  {
                    id: "helpers.ts",
                    data: {
                      name: "helpers.ts",
                      type: "file",
                      size: "8 KB",
                      modified: "2024-01-13",
                      permissions: "rw-r--r--",
                    },
                  },
                ],
              },
            ],
          },
          {
            id: "public",
            data: {
              name: "public",
              type: "folder",
              size: "1.3 MB",
              modified: "2024-01-15",
              permissions: "rwxr-xr-x",
            },
            children: [
              {
                id: "index.html",
                data: {
                  name: "index.html",
                  type: "file",
                  size: "2 KB",
                  modified: "2024-01-15",
                  permissions: "rw-r--r--",
                },
              },
            ],
          },
        ],
      },
    ];

    const fileColumns = [
      {
        key: "name",
        title: "Name",
        dataIndex: "name" as keyof (typeof fileSystemData)[0]["data"],
        sortable: true,
        render: (value: string, node: any, level: number) => (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: "8px",
              paddingLeft: `${level * 8}px`,
            }}
          >
            <Icon
              name={node.data.type === "folder" ? "folder" : "file"}
              size="xs"
              style={{
                color:
                  node.data.type === "folder"
                    ? tokens.color.brand[600].value
                    : tokens.color.neutral[400].value,
              }}
            />
            <span>{value}</span>
          </div>
        ),
      },
      {
        key: "type",
        title: "Type",
        dataIndex: "type" as keyof (typeof fileSystemData)[0]["data"],
        sortable: true,
        render: (value: string) => (
          <span
            style={{
              padding: "4px 8px",
              borderRadius: "4px",
              fontSize: "12px",
              fontWeight: "500",
              background:
                value === "folder"
                  ? "var(--color-brand-600)20"
                  : "var(--color-background-secondary)",
              color: value === "folder" ? "var(--color-brand-600)" : "#FFFFFF",
            }}
          >
            {value}
          </span>
        ),
      },
      {
        key: "size",
        title: "Size",
        dataIndex: "size" as keyof (typeof fileSystemData)[0]["data"],
        sortable: true,
        align: "right" as const,
      },
      {
        key: "modified",
        title: "Modified",
        dataIndex: "modified" as keyof (typeof fileSystemData)[0]["data"],
        sortable: true,
      },
      {
        key: "permissions",
        title: "Permissions",
        dataIndex: "permissions" as keyof (typeof fileSystemData)[0]["data"],
        sortable: true,
        render: (value: string) => (
          <code
            style={{
              background: "var(--color-background-secondary)",
              padding: "2px 6px",
              borderRadius: "3px",
              fontSize: "11px",
              color: tokens.color.neutral[300].value,
            }}
          >
            {value}
          </code>
        ),
      },
    ];

    return (
      <div>
        <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          File System Tree
        </h3>
        <TreeTable
          data={fileSystemData}
          columns={fileColumns}
          searchable={true}
          sortable={true}
          selectable={true}
          showExpandAll={true}
          defaultExpandAll={false}
        />
      </div>
    );
  },
};
