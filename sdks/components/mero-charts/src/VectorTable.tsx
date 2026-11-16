import React from "react";

export type VectorRow = {
  name: string;
  mean: number;
  last: number;
  max: number;
  color?: string;
  [key: string]: any; // Allow dynamic properties
};

export type VectorTableProps = {
  title: string;
  data: VectorRow[];
  colors?: string[];
};

export type ColumnConfig = {
  title: string;
  function: (series: any) => number;
};

export function VectorTableRaw({
  data,
  colors,
  columns,
}: {
  data: VectorRow[];
  colors?: string[];
  columns?: ColumnConfig[];
}) {
  // Default columns if not provided
  const defaultColumns = [
    { title: "Mean", function: () => 0 },
    { title: "Last *", function: () => 0 },
    { title: "Max", function: () => 0 },
  ];

  const finalColumns = columns || defaultColumns;

  return (
    <div style={{ overflow: "auto" }}>
      <table style={{ width: "100%", fontSize: "14px" }}>
        <thead>
          <tr style={{ textAlign: "left", color: "#94a3b8" }}>
            <th style={{ padding: "8px 12px 8px 0", fontWeight: "500" }}>
              Name
            </th>
            {finalColumns.map((column, index) => (
              <th
                key={index}
                style={{
                  padding: "8px 12px",
                  fontWeight: "500",
                  textAlign: "right",
                }}
              >
                {column.title}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {data.map((row, index) => (
            <tr key={index} style={{ borderTop: "1px solid #374151" }}>
              <td style={{ padding: "8px 12px 8px 0", position: "relative" }}>
                <div
                  style={{
                    position: "absolute",
                    left: "0",
                    top: "50%",
                    transform: "translateY(-50%)",
                    width: "4px",
                    height: "12px",
                    backgroundColor: colors?.[index] || row.color || "#A5FF11",
                    borderRadius: "2px",
                  }}
                />
                <span style={{ marginLeft: "16px", color: "white" }}>
                  {row.name}
                </span>
              </td>
              {finalColumns.map((column, colIndex) => {
                const key = column.title.toLowerCase().replace(/\s+/g, "_");
                const value = row[key] || 0;
                return (
                  <td
                    key={colIndex}
                    style={{
                      padding: "8px 12px",
                      textAlign: "right",
                      color: "white",
                      fontVariantNumeric: "tabular-nums",
                    }}
                  >
                    {typeof value === "number" ? value.toFixed(3) : value}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
