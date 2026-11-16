import type { Meta, StoryObj } from "@storybook/react-vite";
import { Table, Button } from "@calimero-network/mero-ui";
import {
  Settings as EditIcon,
  Trash as TrashIcon,
} from "@calimero-network/mero-icons";

type Person = { name: string; email: string; role: string; lastActive: string };

const columns: any[] = [
  { key: "name", header: "Name" },
  { key: "email", header: "Email" },
  { key: "role", header: "Role" },
  { key: "lastActive", header: "Last Active", align: "right" },
];

const data: Person[] = [
  {
    name: "Alice Johnson",
    email: "alice@example.com",
    role: "Admin",
    lastActive: "2h ago",
  },
  {
    name: "Bob Smith",
    email: "bob@example.com",
    role: "User",
    lastActive: "1d ago",
  },
  {
    name: "Carol Lee",
    email: "carol@example.com",
    role: "Manager",
    lastActive: "3d ago",
  },
];

const meta: Meta<typeof Table> = {
  title: "Data Display/Table",
  component: Table,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof meta>;

export const Basic: Story = {
  render: () => (
    <div style={{ width: 720 }}>
      <Table columns={columns} data={data} />
    </div>
  ),
};

export const CompactSticky: Story = {
  render: () => (
    <div style={{ width: 720, maxHeight: 240 }}>
      <Table columns={columns} data={data} compact stickyHeader />
    </div>
  ),
};

export const WithControls: Story = {
  argTypes: {
    showEdit: { control: "boolean" },
    showDelete: { control: "boolean" },
    useButtons: { control: "boolean" },
    compact: { control: "boolean" },
  },
  args: {
    showEdit: true,
    showDelete: true,
    useButtons: false,
    compact: false,
  },
  render: (args: any) => {
    const dataLocal: Person[] = [
      {
        name: "Alice Johnson",
        email: "alice@example.com",
        role: "Admin",
        lastActive: "2h ago",
      },
      {
        name: "Bob Smith",
        email: "bob@example.com",
        role: "User",
        lastActive: "1d ago",
      },
      {
        name: "Carol Lee",
        email: "carol@example.com",
        role: "Manager",
        lastActive: "3d ago",
      },
    ];

    const baseColumns = [
      { key: "name", header: "Name" },
      { key: "email", header: "Email" },
      { key: "role", header: "Role" },
      { key: "lastActive", header: "Last Active", align: "right" as const },
    ];

    const makeActions = (row: Person) => {
      const items: React.ReactNode[] = [];
      if (args.showEdit) {
        items.push(
          args.useButtons ? (
            <Button
              key="edit"
              variant="info"
              onClick={() => alert(`Edit ${row.name}`)}
              style={
                args.compact
                  ? {
                      height: 28,
                      padding: "0 10px",
                      borderRadius: 8,
                      fontSize: 12,
                    }
                  : undefined
              }
            >
              <EditIcon size={args.compact ? 14 : 16} />
              {args.compact ? " Edit" : " Edit"}
            </Button>
          ) : (
            <span
              key="edit"
              title={`Edit ${row.name}`}
              onClick={() => alert(`Edit ${row.name}`)}
              style={{
                display: "inline-grid",
                placeItems: "center",
                width: args.compact ? 22 : 28,
                height: args.compact ? 22 : 28,
                borderRadius: args.compact ? 6 : 8,
                cursor: "pointer",
                color: "var(--color-semantic-info)",
              }}
            >
              <EditIcon size={args.compact ? 14 : 18} />
            </span>
          ),
        );
      }
      if (args.showDelete) {
        items.push(
          args.useButtons ? (
            <Button
              key="delete"
              variant="error"
              onClick={() => alert(`Delete ${row.name}`)}
              style={
                args.compact
                  ? {
                      height: 28,
                      padding: "0 10px",
                      borderRadius: 8,
                      fontSize: 12,
                    }
                  : undefined
              }
            >
              <TrashIcon size={args.compact ? 14 : 16} />
              {args.compact ? " Delete" : " Delete"}
            </Button>
          ) : (
            <span
              key="delete"
              title={`Delete ${row.name}`}
              onClick={() => alert(`Delete ${row.name}`)}
              style={{
                display: "inline-grid",
                placeItems: "center",
                width: args.compact ? 22 : 28,
                height: args.compact ? 22 : 28,
                borderRadius: args.compact ? 6 : 8,
                cursor: "pointer",
                color: "var(--color-semantic-error)",
              }}
            >
              <TrashIcon size={args.compact ? 14 : 18} />
            </span>
          ),
        );
      }
      return (
        <div
          style={{
            display: "flex",
            gap: args.compact ? 8 : 12,
            justifyContent: "flex-end",
            alignItems: "center",
          }}
        >
          {items}
        </div>
      );
    };

    const cols: any[] = [
      ...baseColumns,
      {
        key: "__actions__",
        header: "Actions",
        align: "right" as const,
        render: (_: any, row: Person) =>
          args.showEdit || args.showDelete ? makeActions(row) : <div />,
      },
    ];

    return (
      <div style={{ width: args.compact ? 720 : 780 }}>
        <Table
          columns={cols as any}
          data={dataLocal as any}
          compact={args.compact}
        />
      </div>
    );
  },
} as any;
