import React, { useState, useMemo, useCallback, useRef } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Button } from "./Button";
import { Input } from "./Input";
import { Select, SelectOption } from "./Select";
import { NumberInput } from "./NumberInput";
import { Textarea } from "./Textarea";
import { Pagination } from "./Pagination";

// Base column type
export type TableColumn<T> = {
  key: keyof T;
  title: string;
  width?: number | string;
  align?: "left" | "center" | "right";
  render?: (
    value: any,
    row: T,
    rowIndex: number,
    isEditing?: boolean,
  ) => React.ReactNode;
  // Data management features
  sortable?: boolean;
  filterable?: boolean;
  fixed?: "left" | "right";
  // Editing features
  editable?: boolean;
  type?: "text" | "number" | "select" | "textarea" | "date" | "email" | "url";
  options?: SelectOption[]; // For select type
  placeholder?: string;
  required?: boolean;
  min?: number; // For number type
  max?: number; // For number type
  step?: number; // For number type
  precision?: number; // For number type
  rows?: number; // For textarea type
  validate?: (value: any, row: T) => string | null; // Return error message or null
};

// Table modes
export type TableMode = "display" | "data" | "editable";

// Base table props
export interface BaseTableProps<T extends Record<string, any>> {
  data: T[];
  columns: TableColumn<T>[];
  mode?: TableMode;
  className?: string;
  style?: React.CSSProperties;
  zebra?: boolean;
  compact?: boolean;
  stickyHeader?: boolean;
  bordered?: boolean;
  striped?: boolean;
  hoverable?: boolean;
  size?: "small" | "medium" | "large";
  rowKey?: keyof T | ((row: T, index: number) => string);
  emptyStateText?: string;
  disabled?: boolean;
  loading?: boolean;
}

// Display mode props (basic table)
export interface DisplayTableProps<T extends Record<string, any>>
  extends BaseTableProps<T> {
  mode?: "display";
}

// Data mode props (with search, sort, pagination, selection)
export interface DataTableProps<T extends Record<string, any>>
  extends BaseTableProps<T> {
  mode: "data";
  searchable?: boolean;
  searchPlaceholder?: string;
  sortable?: boolean;
  filterable?: boolean;
  selectable?: boolean;
  selectedRowKeys?: string[];
  onSelectionChange?: (selectedKeys: string[], selectedRows: T[]) => void;
  onRowClick?: (record: T, index: number) => void;
  onSort?: (column: string, direction: "asc" | "desc") => void;
  onFilter?: (filters: Record<string, any>) => void;
  pagination?: {
    pageSize?: number;
    showSizeChanger?: boolean;
    showQuickJumper?: boolean;
    showTotal?: (total: number, range: [number, number]) => string;
  };
}

// Editable mode props (with inline editing)
export interface EditableTableProps<T extends Record<string, any>>
  extends BaseTableProps<T> {
  mode: "editable";
  onDataChange?: (newData: T[]) => void;
  onRowAdd?: (newRow: T) => void;
  onRowDelete?: (rowIndex: number, row: T) => void;
  onRowUpdate?: (rowIndex: number, updatedRow: T) => void;
  showAddButton?: boolean;
  showDeleteButton?: boolean;
  addButtonText?: string;
  deleteButtonText?: string;
}

// Union type for all table props
export type TableProps<T extends Record<string, any>> =
  | DisplayTableProps<T>
  | DataTableProps<T>
  | EditableTableProps<T>;

export function Table<T extends Record<string, any>>(props: TableProps<T>) {
  const {
    data,
    columns,
    mode = "display",
    className = "",
    style,
    zebra = true,
    compact = false,
    stickyHeader = false,
    bordered = true,
    striped = true,
    hoverable = true,
    size = "medium",
    rowKey = "id",
    emptyStateText = "No data",
    disabled = false,
    loading = false,
  } = props;

  // State for data mode
  const [searchTerm, setSearchTerm] = useState("");
  const [sortColumn, setSortColumn] = useState<string | null>(null);
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [filters, setFilters] = useState<Record<string, any>>({});

  // State for editable mode
  const [editingCell, setEditingCell] = useState<{
    rowIndex: number;
    columnKey: keyof T;
  } | null>(null);
  const [editingValue, setEditingValue] = useState<any>("");
  const [validationErrors, setValidationErrors] = useState<
    Record<string, string>
  >({});
  const [isAddingRow, setIsAddingRow] = useState(false);
  const [newRowData, setNewRowData] = useState<Partial<T>>({});

  const tableRef = useRef<HTMLTableElement>(null);

  // Get row key
  const getRowKey = useCallback(
    (row: T, index: number): string => {
      if (typeof rowKey === "function") {
        return rowKey(row, index);
      }
      return String(row[rowKey] || index);
    },
    [rowKey],
  );

  // Filter data (for data mode)
  const filteredData = useMemo(() => {
    if (mode !== "data") return data;

    let result = data;

    // Apply search
    if (props.mode === "data" && props.searchable && searchTerm) {
      result = result.filter((record) =>
        columns.some((column) => {
          const value = record[column.key];
          return String(value).toLowerCase().includes(searchTerm.toLowerCase());
        }),
      );
    }

    // Apply filters
    Object.entries(filters).forEach(([key, value]) => {
      if (value !== undefined && value !== "") {
        result = result.filter((record) => {
          const recordValue = (record as any)[key];
          return String(recordValue)
            .toLowerCase()
            .includes(String(value).toLowerCase());
        });
      }
    });

    return result;
  }, [data, searchTerm, filters, columns, mode, props]);

  // Sort data (for data mode)
  const sortedData = useMemo(() => {
    if (mode !== "data" || !sortColumn) return filteredData;

    return [...filteredData].sort((a, b) => {
      const aValue = (a as any)[sortColumn];
      const bValue = (b as any)[sortColumn];

      if (aValue === bValue) return 0;

      const comparison = aValue < bValue ? -1 : 1;
      return sortDirection === "asc" ? comparison : -comparison;
    });
  }, [filteredData, sortColumn, sortDirection, mode]);

  // Paginate data (for data mode)
  const paginatedData = useMemo(() => {
    if (mode !== "data") return sortedData;

    const start = (currentPage - 1) * pageSize;
    const end = start + pageSize;
    return sortedData.slice(start, end);
  }, [sortedData, currentPage, pageSize, mode]);

  // Get final data based on mode
  const finalData = mode === "data" ? paginatedData : data;

  // Size styles
  const sizeStyles = {
    small: { padding: "8px 12px", fontSize: "12px" },
    medium: { padding: "12px 16px", fontSize: "14px" },
    large: { padding: "16px 20px", fontSize: "16px" },
  };

  const currentSizeStyle = sizeStyles[size];
  const cellPadding = compact ? "8px 10px" : currentSizeStyle.padding;

  const tableStyles: React.CSSProperties = {
    width: "100%",
    borderCollapse: "separate",
    borderSpacing: 0,
    background: "transparent",
    border: bordered ? "1px solid #404040" : "none",
    borderRadius: "12px",
    overflow: "hidden",
    ...style,
  };

  return (
    <div className={className} style={{ width: "100%" }}>
      {/* Table */}
      <div style={{ width: "100%", overflowX: "auto" }}>
        <table ref={tableRef} style={tableStyles}>
          {/* Header */}
          <thead
            style={{
              position: stickyHeader ? ("sticky" as const) : "static",
              top: 0,
              zIndex: 1,
            }}
          >
            <tr>
              {columns.map((col, idx) => (
                <th
                  key={String(col.key) + idx}
                  style={{
                    textAlign: col.align || "left",
                    padding: cellPadding,
                    color: "#D1D5DB",
                    fontWeight: 600,
                    fontSize: 12,
                    fontFamily: "var(--font-body)",
                    background: "transparent",
                    borderBottom: "1px solid #2A2A2A",
                    position: stickyHeader ? "sticky" : "static",
                    top: 0,
                    width: col.width,
                  }}
                >
                  {col.title}
                  {col.required && (
                    <span
                      style={{
                        color: tokens.color.semantic.error.value,
                        marginLeft: "4px",
                      }}
                    >
                      *
                    </span>
                  )}
                </th>
              ))}
            </tr>
          </thead>

          {/* Body */}
          <tbody>
            {finalData.map((row, rowIndex) => {
              const key = getRowKey(row, rowIndex);

              return (
                <tr
                  key={key}
                  style={{
                    background:
                      striped && rowIndex % 2 === 1
                        ? "rgba(255, 255, 255, 0.02)"
                        : "transparent",
                  }}
                >
                  {columns.map((col, colIndex) => {
                    const value = row[col.key];

                    return (
                      <td
                        key={String(col.key) + colIndex}
                        style={{
                          textAlign: col.align || "left",
                          padding: cellPadding,
                          color: "white",
                          fontSize: 13,
                          fontFamily: "var(--font-body)",
                          borderBottom: "1px solid #2A2A2A",
                          width: col.width,
                          whiteSpace: "nowrap",
                        }}
                      >
                        {col.render
                          ? col.render(value, row, rowIndex, false)
                          : String(value || "")}
                      </td>
                    );
                  })}
                </tr>
              );
            })}

            {/* Empty State */}
            {finalData.length === 0 && !isAddingRow && (
              <tr>
                <td
                  colSpan={columns.length}
                  style={{
                    padding: cellPadding,
                    color: "#9CA3AF",
                    textAlign: "center",
                  }}
                >
                  {loading ? "Loading..." : emptyStateText}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// Convenience exports for backward compatibility
export type { TableColumn as DataTableColumn };
export type { TableColumn as EditableTableColumn };

// Convenience components for backward compatibility
export const DataTable = <T extends Record<string, any>>(
  props: Omit<DataTableProps<T>, "mode">,
) => <Table {...props} mode="data" />;

export const EditableTable = <T extends Record<string, any>>(
  props: Omit<EditableTableProps<T>, "mode">,
) => <Table {...props} mode="editable" />;
