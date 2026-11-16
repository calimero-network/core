import React from "react";

export type ListItem = {
  id: string | number;
  content: React.ReactNode;
  icon?: React.ReactNode;
  hint?: React.ReactNode;
};

type ListProps = {
  items: ListItem[];
  variant?: "solid" | "ghost";
  divider?: boolean;
  className?: string;
  style?: React.CSSProperties;
};

export function List({
  items,
  variant = "solid",
  divider = false,
  className = "",
  style,
}: ListProps) {
  const containerStyles: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    gap: divider ? "0px" : "8px",
    background: variant === "solid" ? "#1A1A1A" : "transparent",
    border: variant === "solid" ? "1px solid #404040" : "none",
    borderRadius: "12px",
    padding: variant === "solid" ? "8px" : 0,
    ...style,
  };

  return (
    <div className={className} style={containerStyles}>
      {items.map((item, index) => (
        <div
          key={item.id}
          style={{
            display: "flex",
            alignItems: "center",
            gap: "12px",
            padding: "10px 12px",
            color: "white",
            borderBottom:
              divider && index < items.length - 1
                ? "1px solid #2A2A2A"
                : "none",
          }}
        >
          {item.icon && (
            <div
              style={{
                display: "grid",
                placeItems: "center",
                width: 20,
                height: 20,
              }}
            >
              {item.icon}
            </div>
          )}
          <div style={{ flex: 1, minWidth: 0 }}>{item.content}</div>
          {item.hint && (
            <div style={{ color: "#A3A3A3", fontSize: 12 }}>{item.hint}</div>
          )}
        </div>
      ))}
    </div>
  );
}
