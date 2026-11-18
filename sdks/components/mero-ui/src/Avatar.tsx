import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface AvatarProps {
  src?: string;
  alt?: string;
  name?: string;
  size?: "xs" | "sm" | "md" | "lg" | "xl" | "2xl";
  shape?: "circle" | "square";
  fallback?: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export const Avatar: React.FC<AvatarProps> = ({
  src,
  alt,
  name,
  size = "md",
  shape = "circle",
  fallback,
  className = "",
  style = {},
}) => {
  const sizeMap = {
    xs: 24,
    sm: 32,
    md: 40,
    lg: 48,
    xl: 64,
    "2xl": 80,
  };

  const avatarSize = sizeMap[size];

  const baseStyle = {
    width: avatarSize,
    height: avatarSize,
    borderRadius: shape === "circle" ? "50%" : tokens.radius.md.value,
    backgroundColor: tokens.color.neutral[600].value,
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    overflow: "hidden",
    flexShrink: 0,
    ...style,
  };

  const imageStyle = {
    width: "100%",
    height: "100%",
    objectFit: "cover" as const,
  };

  const fallbackStyle = {
    fontSize: avatarSize * 0.4,
    fontWeight: 600,
    color: tokens.color.neutral[200].value,
    textTransform: "uppercase" as const,
  };

  const getInitials = (name: string) => {
    return name
      .split(" ")
      .map((word) => word.charAt(0))
      .join("")
      .slice(0, 2);
  };

  const renderFallback = () => {
    if (fallback) {
      return fallback;
    }

    if (name) {
      return <span style={fallbackStyle}>{getInitials(name)}</span>;
    }

    return (
      <svg
        width={avatarSize * 0.5}
        height={avatarSize * 0.5}
        viewBox="0 0 24 24"
        fill="none"
        style={{ color: tokens.color.neutral[200].value }}
      >
        <path
          d="M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z"
          fill="currentColor"
        />
      </svg>
    );
  };

  return (
    <div className={className} style={baseStyle}>
      {src ? (
        <img
          src={src}
          alt={alt || name || "Avatar"}
          style={imageStyle}
          onError={(e) => {
            // Hide image on error and show fallback
            e.currentTarget.style.display = "none";
          }}
        />
      ) : null}
      {!src && renderFallback()}
    </div>
  );
};
