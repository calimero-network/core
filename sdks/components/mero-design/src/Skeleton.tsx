import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface SkeletonProps {
  width?: string | number;
  height?: string | number;
  variant?: "text" | "rectangular" | "circular" | "rounded";
  animation?: "pulse" | "wave" | "none";
  lines?: number;
  className?: string;
  style?: React.CSSProperties;
}

export function Skeleton({
  width = "100%",
  height = "20px",
  variant = "rectangular",
  animation = "pulse",
  lines = 1,
  className = "",
  style = {},
}: SkeletonProps) {
  const baseStyle: React.CSSProperties = {
    background: `linear-gradient(90deg, ${tokens.color.neutral[700].value} 25%, ${tokens.color.neutral[600].value} 50%, ${tokens.color.neutral[700].value} 75%)`,
    backgroundSize: "200% 100%",
    width: typeof width === "number" ? `${width}px` : width,
    height: typeof height === "number" ? `${height}px` : height,
    borderRadius:
      variant === "circular" ? "50%" : variant === "rounded" ? "8px" : "4px",
    display: "inline-block",
    ...style,
  };

  const animationStyle: React.CSSProperties = {
    animation:
      animation === "pulse"
        ? "skeleton-pulse 1.5s ease-in-out infinite"
        : animation === "wave"
          ? "skeleton-wave 1.6s ease-in-out infinite"
          : "none",
  };

  // Create keyframes for animations
  const keyframes = `
    @keyframes skeleton-pulse {
      0% {
        opacity: 1;
      }
      50% {
        opacity: 0.4;
      }
      100% {
        opacity: 1;
      }
    }
    
    @keyframes skeleton-wave {
      0% {
        background-position: -200% 0;
      }
      100% {
        background-position: 200% 0;
      }
    }
  `;

  // Add keyframes to document if not already added
  React.useEffect(() => {
    const styleId = "skeleton-keyframes";
    if (!document.getElementById(styleId)) {
      const style = document.createElement("style");
      style.id = styleId;
      style.textContent = keyframes;
      document.head.appendChild(style);
    }
  }, []);

  if (lines > 1) {
    return (
      <div className={className}>
        {Array.from({ length: lines }, (_, index) => (
          <div
            key={index}
            style={{
              ...baseStyle,
              ...animationStyle,
              marginBottom: index < lines - 1 ? "8px" : "0",
              width: index === lines - 1 ? "75%" : "100%", // Last line is shorter
            }}
          />
        ))}
      </div>
    );
  }

  return (
    <div
      className={className}
      style={{
        ...baseStyle,
        ...animationStyle,
      }}
    />
  );
}

// Preset skeleton components for common use cases
export function SkeletonText({
  lines = 3,
  ...props
}: Omit<SkeletonProps, "variant" | "height">) {
  return (
    <Skeleton {...props} variant="rectangular" height="16px" lines={lines} />
  );
}

export function SkeletonTitle({
  ...props
}: Omit<SkeletonProps, "variant" | "height">) {
  return (
    <Skeleton {...props} variant="rectangular" height="24px" width="60%" />
  );
}

export function SkeletonAvatar({
  size = 40,
  ...props
}: Omit<SkeletonProps, "variant" | "width" | "height"> & { size?: number }) {
  return <Skeleton {...props} variant="circular" width={size} height={size} />;
}

export function SkeletonButton({
  ...props
}: Omit<SkeletonProps, "variant" | "height">) {
  return <Skeleton {...props} variant="rounded" height="40px" width="120px" />;
}

export function SkeletonCard({
  ...props
}: Omit<SkeletonProps, "variant" | "height" | "width">) {
  return (
    <div
      style={{
        padding: "16px",
        background: "var(--color-background-primary)",
        borderRadius: "8px",
        border: `1px solid ${tokens.color.neutral[600].value}`,
      }}
    >
      <SkeletonTitle style={{ marginBottom: "12px" }} />
      <SkeletonText lines={2} style={{ marginBottom: "16px" }} />
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <SkeletonButton width="80px" />
        <SkeletonAvatar size={32} />
      </div>
    </div>
  );
}

export function SkeletonTable({
  rows = 5,
  columns = 4,
  ...props
}: Omit<SkeletonProps, "variant" | "height" | "width" | "lines"> & {
  rows?: number;
  columns?: number;
}) {
  return (
    <div
      style={{
        border: `1px solid ${tokens.color.neutral[600].value}`,
        borderRadius: "8px",
        overflow: "hidden",
      }}
    >
      {/* Header */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: `repeat(${columns}, 1fr)`,
          gap: "16px",
          padding: "12px 16px",
          background: "var(--color-background-secondary)",
          borderBottom: `1px solid ${tokens.color.neutral[600].value}`,
        }}
      >
        {Array.from({ length: columns }, (_, index) => (
          <Skeleton
            key={`header-${index}`}
            variant="rectangular"
            height="16px"
            width="80%"
            {...props}
          />
        ))}
      </div>

      {/* Rows */}
      {Array.from({ length: rows }, (_, rowIndex) => (
        <div
          key={`row-${rowIndex}`}
          style={{
            display: "grid",
            gridTemplateColumns: `repeat(${columns}, 1fr)`,
            gap: "16px",
            padding: "12px 16px",
            borderBottom:
              rowIndex < rows - 1
                ? `1px solid ${tokens.color.neutral[700].value}`
                : "none",
          }}
        >
          {Array.from({ length: columns }, (_, colIndex) => (
            <Skeleton
              key={`cell-${rowIndex}-${colIndex}`}
              variant="rectangular"
              height="14px"
              width={colIndex === columns - 1 ? "60%" : "90%"}
              {...props}
            />
          ))}
        </div>
      ))}
    </div>
  );
}
