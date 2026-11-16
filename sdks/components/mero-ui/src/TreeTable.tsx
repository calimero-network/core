import React, { useState, useMemo, useCallback } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";
import { Button } from "./Button";
import { Input } from "./Input";

export interface TreeNode<T = any> {
  id: string;
  data: T;
  children?: TreeNode<T>[];
  expanded?: boolean;
  level?: number;
  parent?: TreeNode<T>;
}

export interface TreeTableColumn<T = any> {
  key: string;
  title: string;
  dataIndex?: keyof T;
  render?: (value: any, node: TreeNode<T>, level: number) => React.ReactNode;
  width?: string | number;
  align?: "left" | "center" | "right";
  sortable?: boolean;
  filterable?: boolean;
}

export interface TreeTableProps<T = any> {
  data: TreeNode<T>[];
  columns: TreeTableColumn<T>[];
  loading?: boolean;
  searchable?: boolean;
  searchPlaceholder?: string;
  sortable?: boolean;
  filterable?: boolean;
  selectable?: boolean;
  selectedKeys?: string[];
  onSelectionChange?: (
    selectedKeys: string[],
    selectedNodes: TreeNode<T>[],
  ) => void;
  onNodeClick?: (node: TreeNode<T>, level: number) => void;
  onNodeToggle?: (node: TreeNode<T>, expanded: boolean) => void;
  onSort?: (column: string, direction: "asc" | "desc") => void;
  onFilter?: (filters: Record<string, any>) => void;
  className?: string;
  style?: React.CSSProperties;
  size?: "small" | "medium" | "large";
  bordered?: boolean;
  striped?: boolean;
  hoverable?: boolean;
  expandIcon?: React.ReactNode;
  collapseIcon?: React.ReactNode;
  indentSize?: number;
  showExpandAll?: boolean;
  defaultExpandAll?: boolean;
}

export function TreeTable<T = any>({
  data,
  columns,
  loading = false,
  searchable = true,
  searchPlaceholder = "Search...",
  sortable = true,
  filterable = false,
  selectable = false,
  selectedKeys = [],
  onSelectionChange,
  onNodeClick,
  onNodeToggle,
  onSort,
  onFilter,
  className = "",
  style = {},
  size = "medium",
  bordered = true,
  striped = true,
  hoverable = true,
  expandIcon,
  collapseIcon,
  indentSize = 20,
  showExpandAll = true,
  defaultExpandAll = false,
}: TreeTableProps<T>) {
  const [searchTerm, setSearchTerm] = useState("");
  const [sortColumn, setSortColumn] = useState<string | null>(null);
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [filters, setFilters] = useState<Record<string, any>>({});
  const [expandedKeys, setExpandedKeys] = useState<Set<string>>(
    defaultExpandAll ? new Set(data.map((node) => node.id)) : new Set(),
  );

  // Flatten tree data for display
  const flattenedData = useMemo(() => {
    const flatten = (nodes: TreeNode<T>[], level = 0): TreeNode<T>[] => {
      const result: TreeNode<T>[] = [];

      for (const node of nodes) {
        const flattenedNode = { ...node, level };
        result.push(flattenedNode);

        if (
          node.children &&
          node.children.length > 0 &&
          expandedKeys.has(node.id)
        ) {
          result.push(...flatten(node.children, level + 1));
        }
      }

      return result;
    };

    return flatten(data);
  }, [data, expandedKeys]);

  // Filter data
  const filteredData = useMemo(() => {
    let result = flattenedData;

    // Apply search
    if (searchTerm) {
      result = result.filter((node) =>
        columns.some((column) => {
          const value = column.dataIndex
            ? (node.data as any)[column.dataIndex]
            : "";
          return String(value).toLowerCase().includes(searchTerm.toLowerCase());
        }),
      );
    }

    // Apply filters
    Object.entries(filters).forEach(([key, value]) => {
      if (value !== undefined && value !== "") {
        result = result.filter((node) => {
          const recordValue = (node.data as any)[key];
          return String(recordValue)
            .toLowerCase()
            .includes(String(value).toLowerCase());
        });
      }
    });

    return result;
  }, [flattenedData, searchTerm, filters, columns]);

  // Sort data
  const sortedData = useMemo(() => {
    if (!sortColumn) return filteredData;

    return [...filteredData].sort((a, b) => {
      const aValue = (a.data as any)[sortColumn];
      const bValue = (b.data as any)[sortColumn];

      if (aValue === bValue) return 0;

      const comparison = aValue < bValue ? -1 : 1;
      return sortDirection === "asc" ? comparison : -comparison;
    });
  }, [filteredData, sortColumn, sortDirection]);

  // Handle expand/collapse
  const handleToggle = useCallback(
    (node: TreeNode<T>) => {
      const newExpandedKeys = new Set(expandedKeys);

      if (expandedKeys.has(node.id)) {
        newExpandedKeys.delete(node.id);
      } else {
        newExpandedKeys.add(node.id);
      }

      setExpandedKeys(newExpandedKeys);
      onNodeToggle?.(node, !expandedKeys.has(node.id));
    },
    [expandedKeys, onNodeToggle],
  );

  // Handle expand all
  const handleExpandAll = useCallback(() => {
    const allKeys = new Set<string>();
    const collectKeys = (nodes: TreeNode<T>[]) => {
      for (const node of nodes) {
        allKeys.add(node.id);
        if (node.children) {
          collectKeys(node.children);
        }
      }
    };
    collectKeys(data);
    setExpandedKeys(allKeys);
  }, [data]);

  // Handle collapse all
  const handleCollapseAll = useCallback(() => {
    setExpandedKeys(new Set());
  }, []);

  // Handle sort
  const handleSort = useCallback(
    (column: string) => {
      if (!sortable) return;

      const columnConfig = columns.find((col) => col.key === column);
      if (!columnConfig?.sortable) return;

      if (sortColumn === column) {
        setSortDirection(sortDirection === "asc" ? "desc" : "asc");
      } else {
        setSortColumn(column);
        setSortDirection("asc");
      }

      onSort?.(
        column,
        sortColumn === column
          ? sortDirection === "asc"
            ? "desc"
            : "asc"
          : "asc",
      );
    },
    [sortable, columns, sortColumn, sortDirection, onSort],
  );

  // Handle selection
  const handleSelectNode = useCallback(
    (node: TreeNode<T>, checked: boolean) => {
      if (!selectable || !onSelectionChange) return;

      const newSelectedKeys = checked
        ? [...selectedKeys, node.id]
        : selectedKeys.filter((key) => key !== node.id);

      const newSelectedNodes = data.filter((node) =>
        newSelectedKeys.includes(node.id),
      );

      onSelectionChange(newSelectedKeys, newSelectedNodes);
    },
    [selectable, onSelectionChange, selectedKeys, data],
  );

  // Size styles
  const sizeStyles = {
    small: { padding: "8px 12px", fontSize: "12px" },
    medium: { padding: "12px 16px", fontSize: "14px" },
    large: { padding: "16px 20px", fontSize: "16px" },
  };

  const currentSizeStyle = sizeStyles[size];

  return (
    <div className={className} style={style}>
      {/* Search and Controls */}
      {(searchable || filterable || showExpandAll) && (
        <div
          style={{
            marginBottom: "16px",
            display: "flex",
            gap: "12px",
            alignItems: "center",
            flexWrap: "wrap",
          }}
        >
          {searchable && (
            <div style={{ flex: 1, maxWidth: "300px" }}>
              <Input
                placeholder={searchPlaceholder}
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                style={{ width: "100%" }}
              />
            </div>
          )}

          {showExpandAll && (
            <div style={{ display: "flex", gap: "8px" }}>
              <Button variant="secondary" onClick={handleExpandAll}>
                Expand All
              </Button>
              <Button variant="secondary" onClick={handleCollapseAll}>
                Collapse All
              </Button>
            </div>
          )}

          {filterable && (
            <div style={{ display: "flex", gap: "8px" }}>
              {columns
                .filter((col) => col.filterable)
                .map((column) => (
                  <select
                    key={column.key}
                    value={filters[column.key] || ""}
                    onChange={(e) => {
                      const newFilters = {
                        ...filters,
                        [column.key]: e.target.value,
                      };
                      setFilters(newFilters);
                      onFilter?.(newFilters);
                    }}
                    style={{
                      background: "var(--color-background-secondary)",
                      border: `1px solid ${tokens.color.neutral[600].value}`,
                      borderRadius: "4px",
                      color: "#FFFFFF",
                      padding: "8px 12px",
                      fontSize: "14px",
                      minWidth: "120px",
                    }}
                  >
                    <option value="">All</option>
                    {Array.from(
                      new Set(
                        data.map(
                          (node) =>
                            (node.data as any)[column.dataIndex as keyof T],
                        ),
                      ),
                    ).map((value) => (
                      <option key={String(value)} value={String(value)}>
                        {String(value)}
                      </option>
                    ))}
                  </select>
                ))}
            </div>
          )}
        </div>
      )}

      {/* Table */}
      <div
        style={{
          overflow: "auto",
          border: bordered
            ? `1px solid ${tokens.color.neutral[600].value}`
            : "none",
          borderRadius: "8px",
        }}
      >
        <table
          style={{
            width: "100%",
            borderCollapse: "collapse",
            background: "transparent",
          }}
        >
          <thead>
            <tr style={{ background: "var(--color-background-secondary)" }}>
              {selectable && (
                <th
                  style={{
                    ...currentSizeStyle,
                    width: "50px",
                    textAlign: "center",
                    borderBottom: `1px solid ${tokens.color.neutral[600].value}`,
                    color: "#FFFFFF",
                    fontWeight: 600,
                  }}
                >
                  <input
                    type="checkbox"
                    checked={
                      sortedData.length > 0 &&
                      sortedData.every((node) => selectedKeys.includes(node.id))
                    }
                    onChange={(e) => {
                      if (e.target.checked) {
                        const allKeys = sortedData.map((node) => node.id);
                        onSelectionChange?.(allKeys, sortedData);
                      } else {
                        onSelectionChange?.([], []);
                      }
                    }}
                    style={{ margin: 0 }}
                  />
                </th>
              )}
              {columns.map((column) => (
                <th
                  key={column.key}
                  style={{
                    ...currentSizeStyle,
                    width: column.width,
                    textAlign: column.align || "left",
                    borderBottom: `1px solid ${tokens.color.neutral[600].value}`,
                    color: "#FFFFFF",
                    fontWeight: 600,
                    cursor: column.sortable ? "pointer" : "default",
                    userSelect: "none",
                  }}
                  onClick={() => handleSort(column.key)}
                >
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: "4px",
                    }}
                  >
                    {column.title}
                    {column.sortable && (
                      <div
                        style={{
                          display: "flex",
                          flexDirection: "column",
                          fontSize: "10px",
                        }}
                      >
                        <span
                          style={{
                            color:
                              sortColumn === column.key &&
                              sortDirection === "asc"
                                ? tokens.color.brand[600].value
                                : tokens.color.neutral[400].value,
                          }}
                        >
                          ▲
                        </span>
                        <span
                          style={{
                            color:
                              sortColumn === column.key &&
                              sortDirection === "desc"
                                ? tokens.color.brand[600].value
                                : tokens.color.neutral[400].value,
                          }}
                        >
                          ▼
                        </span>
                      </div>
                    )}
                  </div>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <tr>
                <td
                  colSpan={columns.length + (selectable ? 1 : 0)}
                  style={{
                    ...currentSizeStyle,
                    textAlign: "center",
                    color: tokens.color.neutral[400].value,
                  }}
                >
                  Loading...
                </td>
              </tr>
            ) : sortedData.length === 0 ? (
              <tr>
                <td
                  colSpan={columns.length + (selectable ? 1 : 0)}
                  style={{
                    ...currentSizeStyle,
                    textAlign: "center",
                    color: tokens.color.neutral[400].value,
                  }}
                >
                  No data found
                </td>
              </tr>
            ) : (
              sortedData.map((node, index) => {
                const isSelected = selectedKeys.includes(node.id);
                const hasChildren = node.children && node.children.length > 0;
                const isExpanded = expandedKeys.has(node.id);

                return (
                  <tr
                    key={node.id}
                    style={{
                      background:
                        striped && index % 2 === 1
                          ? "var(--color-background-primary)"
                          : "transparent",
                      cursor: onNodeClick ? "pointer" : "default",
                      opacity: isSelected ? 0.7 : 1,
                    }}
                    onClick={() => onNodeClick?.(node, node.level || 0)}
                    onMouseEnter={(e) => {
                      if (hoverable) {
                        e.currentTarget.style.background =
                          "var(--color-background-secondary)";
                      }
                    }}
                    onMouseLeave={(e) => {
                      if (hoverable) {
                        e.currentTarget.style.background =
                          striped && index % 2 === 1
                            ? "var(--color-background-primary)"
                            : "transparent";
                      }
                    }}
                  >
                    {selectable && (
                      <td
                        style={{
                          ...currentSizeStyle,
                          textAlign: "center",
                          borderBottom: `1px solid ${tokens.color.neutral[700].value}`,
                          color: "#FFFFFF",
                        }}
                      >
                        <input
                          type="checkbox"
                          checked={isSelected}
                          onChange={(e) =>
                            handleSelectNode(node, e.target.checked)
                          }
                          style={{ margin: 0 }}
                        />
                      </td>
                    )}
                    {columns.map((column, colIndex) => (
                      <td
                        key={column.key}
                        style={{
                          ...currentSizeStyle,
                          textAlign: column.align || "left",
                          borderBottom: `1px solid ${tokens.color.neutral[700].value}`,
                          color: "#FFFFFF",
                          paddingLeft:
                            colIndex === 0
                              ? `${(node.level || 0) * indentSize + 12}px`
                              : currentSizeStyle.padding,
                        }}
                      >
                        {colIndex === 0 && hasChildren && (
                          <button
                            type="button"
                            onClick={(e) => {
                              e.stopPropagation();
                              handleToggle(node);
                            }}
                            style={{
                              background: "none",
                              border: "none",
                              cursor: "pointer",
                              marginRight: "8px",
                              padding: "2px",
                              display: "inline-flex",
                              alignItems: "center",
                              color: tokens.color.neutral[400].value,
                            }}
                          >
                            {isExpanded
                              ? collapseIcon || (
                                  <Icon name="chevron-down" size="sm" />
                                )
                              : expandIcon || (
                                  <Icon name="chevron-right" size="sm" />
                                )}
                          </button>
                        )}
                        {colIndex === 0 && !hasChildren && (
                          <span style={{ marginRight: "24px" }} />
                        )}
                        {column.render
                          ? column.render(
                              (node.data as any)[column.dataIndex as keyof T],
                              node,
                              node.level || 0,
                            )
                          : String(
                              (node.data as any)[column.dataIndex as keyof T] ||
                                "",
                            )}
                      </td>
                    ))}
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
