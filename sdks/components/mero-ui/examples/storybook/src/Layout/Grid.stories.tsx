import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Grid,
  GridItem,
  Card,
  CardContent,
  Button,
} from "@calimero-network/mero-ui";
import { cssVariables } from "@calimero-network/mero-tokens";

const withTokens = (Story: any) => (
  <>
    <style>{cssVariables}</style>
    <Story />
  </>
);

const meta: Meta<typeof Grid> = {
  title: "Layout/Grid",
  component: Grid,
  decorators: [withTokens],
  parameters: {
    layout: "padded",
  },
  tags: ["autodocs"],
  argTypes: {
    columns: {
      control: { type: "number", min: 1, max: 24 },
      description: "Number of columns in the grid",
    },
    gap: {
      control: { type: "number", min: 0, max: 48, step: 4 },
      description: "Gap between grid items in pixels",
    },
    maxWidth: {
      control: { type: "text" },
      description: "Maximum width of the grid container",
    },
    justify: {
      control: { type: "select" },
      options: [
        "start",
        "center",
        "end",
        "space-between",
        "space-around",
        "space-evenly",
      ],
      description: "Horizontal alignment of grid content",
    },
    align: {
      control: { type: "select" },
      options: ["start", "center", "end", "stretch"],
      description: "Vertical alignment of grid content",
    },
  },
};

export default meta;

type Story = StoryObj<typeof Grid>;

// Helper component for demo content
const DemoCard = ({
  children,
  color = "#404040",
}: {
  children: React.ReactNode;
  color?: string;
}) => (
  <Card variant="rectangle" color={color} style={{ minHeight: "80px" }}>
    <CardContent>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          height: "100%",
          color: "white",
          fontSize: "14px",
          fontWeight: "500",
        }}
      >
        {children}
      </div>
    </CardContent>
  </Card>
);

export const Basic: Story = {
  args: {
    columns: 12,
    gap: 16,
    maxWidth: "100%",
  },
  render: (args) => (
    <Grid {...args}>
      <GridItem colSpan={6}>
        <DemoCard color="#3B82F6">6 columns</DemoCard>
      </GridItem>
      <GridItem colSpan={6}>
        <DemoCard color="#10B981">6 columns</DemoCard>
      </GridItem>
      <GridItem colSpan={4}>
        <DemoCard color="#F59E0B">4 columns</DemoCard>
      </GridItem>
      <GridItem colSpan={4}>
        <DemoCard color="#EF4444">4 columns</DemoCard>
      </GridItem>
      <GridItem colSpan={4}>
        <DemoCard color="#8B5CF6">4 columns</DemoCard>
      </GridItem>
    </Grid>
  ),
};

export const ResponsiveLayout: Story = {
  render: () => (
    <Grid columns={12} gap={16}>
      <GridItem colSpan={12}>
        <DemoCard color="#1F2937">Full width header</DemoCard>
      </GridItem>
      <GridItem colSpan={8}>
        <DemoCard color="#374151">Main content (8 cols)</DemoCard>
      </GridItem>
      <GridItem colSpan={4}>
        <DemoCard color="#4B5563">Sidebar (4 cols)</DemoCard>
      </GridItem>
      <GridItem colSpan={12}>
        <DemoCard color="#6B7280">Full width footer</DemoCard>
      </GridItem>
    </Grid>
  ),
};

export const CardGrid: Story = {
  render: () => (
    <Grid columns={3} gap={20} maxWidth="800px">
      <GridItem>
        <Card variant="rectangle">
          <CardContent>
            <h3
              style={{ color: "white", margin: "0 0 8px 0", fontSize: "16px" }}
            >
              Feature 1
            </h3>
            <p style={{ color: "#9CA3AF", margin: 0, fontSize: "14px" }}>
              This is a feature card with some description text.
            </p>
          </CardContent>
        </Card>
      </GridItem>
      <GridItem>
        <Card variant="rectangle">
          <CardContent>
            <h3
              style={{ color: "white", margin: "0 0 8px 0", fontSize: "16px" }}
            >
              Feature 2
            </h3>
            <p style={{ color: "#9CA3AF", margin: 0, fontSize: "14px" }}>
              Another feature card with different content.
            </p>
          </CardContent>
        </Card>
      </GridItem>
      <GridItem>
        <Card variant="rectangle">
          <CardContent>
            <h3
              style={{ color: "white", margin: "0 0 8px 0", fontSize: "16px" }}
            >
              Feature 3
            </h3>
            <p style={{ color: "#9CA3AF", margin: 0, fontSize: "14px" }}>
              Third feature card to complete the grid.
            </p>
          </CardContent>
        </Card>
      </GridItem>
    </Grid>
  ),
};

export const ComplexLayout: Story = {
  render: () => (
    <Grid columns={12} gap={16}>
      <GridItem colSpan={12}>
        <DemoCard color="#1F2937">Header - Full Width</DemoCard>
      </GridItem>
      <GridItem colSpan={3}>
        <DemoCard color="#374151">Nav - 3 cols</DemoCard>
      </GridItem>
      <GridItem colSpan={6}>
        <DemoCard color="#4B5563">Main - 6 cols</DemoCard>
      </GridItem>
      <GridItem colSpan={3}>
        <DemoCard color="#6B7280">Aside - 3 cols</DemoCard>
      </GridItem>
      <GridItem colSpan={4}>
        <DemoCard color="#7C3AED">Card 1 - 4 cols</DemoCard>
      </GridItem>
      <GridItem colSpan={4}>
        <DemoCard color="#059669">Card 2 - 4 cols</DemoCard>
      </GridItem>
      <GridItem colSpan={4}>
        <DemoCard color="#DC2626">Card 3 - 4 cols</DemoCard>
      </GridItem>
    </Grid>
  ),
};

export const Alignment: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "32px" }}>
      <div>
        <h3 style={{ color: "white", marginBottom: "16px" }}>
          Center Alignment
        </h3>
        <Grid
          columns={6}
          gap={12}
          justify="center"
          align="center"
          style={{ height: "120px", border: "1px dashed #404040" }}
        >
          <GridItem colSpan={2}>
            <DemoCard color="#3B82F6">Centered</DemoCard>
          </GridItem>
          <GridItem colSpan={2}>
            <DemoCard color="#10B981">Items</DemoCard>
          </GridItem>
        </Grid>
      </div>

      <div>
        <h3 style={{ color: "white", marginBottom: "16px" }}>Space Between</h3>
        <Grid
          columns={6}
          gap={12}
          justify="space-between"
          style={{ height: "120px", border: "1px dashed #404040" }}
        >
          <GridItem colSpan={2}>
            <DemoCard color="#F59E0B">Left</DemoCard>
          </GridItem>
          <GridItem colSpan={2}>
            <DemoCard color="#EF4444">Right</DemoCard>
          </GridItem>
        </Grid>
      </div>
    </div>
  ),
};

export const GridItemProps: Story = {
  render: () => (
    <Grid columns={12} gap={12}>
      <GridItem colSpan={6} colStart={2}>
        <DemoCard color="#3B82F6">colSpan=6, colStart=2</DemoCard>
      </GridItem>
      <GridItem colSpan={4} colStart={8}>
        <DemoCard color="#10B981">colSpan=4, colStart=8</DemoCard>
      </GridItem>
      <GridItem colSpan={3} rowSpan={2}>
        <DemoCard color="#F59E0B">colSpan=3, rowSpan=2</DemoCard>
      </GridItem>
      <GridItem colSpan={6}>
        <DemoCard color="#EF4444">colSpan=6</DemoCard>
      </GridItem>
      <GridItem colSpan={3}>
        <DemoCard color="#8B5CF6">colSpan=3</DemoCard>
      </GridItem>
    </Grid>
  ),
};
