import type { Meta, StoryObj } from "@storybook/react-vite";
import { Pagination } from "@calimero-network/mero-ui";
import React from "react";

const meta: Meta<typeof Pagination> = {
  title: "Navigation/Pagination",
  component: Pagination,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Pagination>;

export const Basic: Story = {
  render: () => {
    const [page, setPage] = React.useState(1);
    return (
      <Pagination currentPage={page} totalPages={10} onPageChange={setPage} />
    );
  },
};

export const WithFirstLast: Story = {
  render: () => {
    const [page, setPage] = React.useState(5);
    return (
      <Pagination
        currentPage={page}
        totalPages={20}
        onPageChange={setPage}
        showFirstLast={true}
        showPrevNext={true}
      />
    );
  },
};

export const ManyPages: Story = {
  render: () => {
    const [page, setPage] = React.useState(50);
    return (
      <Pagination
        currentPage={page}
        totalPages={100}
        onPageChange={setPage}
        maxVisiblePages={7}
      />
    );
  },
};

export const Disabled: Story = {
  render: () => {
    const [page, setPage] = React.useState(3);
    return (
      <Pagination
        currentPage={page}
        totalPages={5}
        onPageChange={setPage}
        disabled={true}
      />
    );
  },
};
