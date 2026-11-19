import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface TextProps {
  children: React.ReactNode;
  size?: "xs" | "sm" | "md" | "lg" | "xl";
  weight?: "normal" | "medium" | "semibold" | "bold";
  color?: "primary" | "secondary" | "muted" | "success" | "warning" | "error";
  align?: "left" | "center" | "right" | "justify";
  as?: "p" | "span" | "div" | "label";
  className?: string;
  style?: React.CSSProperties;
}

export const Text: React.FC<TextProps> = ({
  children,
  size = "md",
  weight = "normal",
  color = "primary",
  align = "left",
  as: Component = "p",
  className = "",
  style = {},
}) => {
  const sizeStyles = {
    xs: { fontSize: "12px", lineHeight: "16px" },
    sm: { fontSize: "14px", lineHeight: "20px" },
    md: { fontSize: "16px", lineHeight: "24px" },
    lg: { fontSize: "18px", lineHeight: "28px" },
    xl: { fontSize: "20px", lineHeight: "32px" },
  };

  const weightStyles = {
    normal: { fontWeight: 400 },
    medium: { fontWeight: 500 },
    semibold: { fontWeight: 600 },
    bold: { fontWeight: 700 },
  };

  const colorStyles = {
    primary: { color: "#FFFFFF" }, // White text for dark theme
    secondary: { color: tokens.color.neutral[300].value },
    muted: { color: tokens.color.neutral[400].value },
    success: { color: tokens.color.semantic.success.value },
    warning: { color: tokens.color.semantic.warning.value },
    error: { color: tokens.color.semantic.error.value },
  };

  const combinedStyle = {
    ...sizeStyles[size],
    ...weightStyles[weight],
    ...colorStyles[color],
    textAlign: align,
    margin: 0,
    ...style,
  };

  return (
    <Component className={className} style={combinedStyle}>
      {children}
    </Component>
  );
};

export interface HeadingProps {
  children: React.ReactNode;
  level?: 1 | 2 | 3 | 4 | 5 | 6;
  size?: "xs" | "sm" | "md" | "lg" | "xl" | "2xl" | "3xl" | "4xl";
  weight?: "normal" | "medium" | "semibold" | "bold";
  color?: "primary" | "secondary" | "muted" | "success" | "warning" | "error";
  align?: "left" | "center" | "right" | "justify";
  className?: string;
  style?: React.CSSProperties;
}

export const Heading: React.FC<HeadingProps> = ({
  children,
  level = 1,
  size,
  weight = "bold",
  color = "primary",
  align = "left",
  className = "",
  style = {},
}) => {
  const Component = `h${level}` as any;

  const sizeMap = {
    xs: { fontSize: "12px", lineHeight: "16px" },
    sm: { fontSize: "14px", lineHeight: "20px" },
    md: { fontSize: "16px", lineHeight: "24px" },
    lg: { fontSize: "18px", lineHeight: "28px" },
    xl: { fontSize: "20px", lineHeight: "32px" },
    "2xl": { fontSize: "24px", lineHeight: "32px" },
    "3xl": { fontSize: "30px", lineHeight: "36px" },
    "4xl": { fontSize: "36px", lineHeight: "40px" },
  };

  const defaultSizes = {
    1: "4xl",
    2: "3xl",
    3: "2xl",
    4: "xl",
    5: "lg",
    6: "md",
  } as const;

  const actualSize = size || defaultSizes[level];

  const sizeStyles = sizeMap[actualSize];

  const weightStyles = {
    normal: { fontWeight: 400 },
    medium: { fontWeight: 500 },
    semibold: { fontWeight: 600 },
    bold: { fontWeight: 700 },
  };

  const colorStyles = {
    primary: { color: "#FFFFFF" }, // White text for dark theme
    secondary: { color: tokens.color.neutral[300].value },
    muted: { color: tokens.color.neutral[400].value },
    success: { color: tokens.color.semantic.success.value },
    warning: { color: tokens.color.semantic.warning.value },
    error: { color: tokens.color.semantic.error.value },
  };

  const combinedStyle = {
    ...sizeStyles,
    ...weightStyles[weight],
    ...colorStyles[color],
    textAlign: align,
    fontFamily: "var(--font-heading)",
    margin: 0,
    ...style,
  };

  return (
    <Component className={className} style={combinedStyle}>
      {children}
    </Component>
  );
};

export interface CodeProps {
  children: React.ReactNode;
  variant?: "inline" | "block";
  language?: string;
  className?: string;
  style?: React.CSSProperties;
}

export const Code: React.FC<CodeProps> = ({
  children,
  variant = "inline",
  language,
  className = "",
  style = {},
}) => {
  const baseStyle = {
    fontFamily: 'Monaco, Consolas, "Courier New", monospace',
    fontSize: "14px",
    backgroundColor: tokens.color.background.secondary.value,
    color: "#FFFFFF", // White text for dark theme
    padding: variant === "inline" ? "2px 6px" : "12px 16px",
    borderRadius: tokens.radius.sm.value,
    border: `1px solid ${tokens.color.neutral[600].value}`,
    ...style,
  };

  const blockStyle = {
    ...baseStyle,
    display: "block",
    whiteSpace: "pre-wrap" as const,
    overflow: "auto",
    margin: "8px 0",
  };

  const inlineStyle = {
    ...baseStyle,
    display: "inline",
  };

  if (variant === "block") {
    return (
      <pre className={className} style={blockStyle}>
        <code>{children}</code>
      </pre>
    );
  }

  return (
    <code className={className} style={inlineStyle}>
      {children}
    </code>
  );
};

export interface LinkProps {
  children: React.ReactNode;
  href?: string;
  onClick?: () => void;
  variant?: "default" | "subtle" | "underline";
  color?: "primary" | "secondary" | "success" | "warning" | "error";
  disabled?: boolean;
  external?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export const Link: React.FC<LinkProps> = ({
  children,
  href,
  onClick,
  variant = "default",
  color = "primary",
  disabled = false,
  external = false,
  className = "",
  style = {},
}) => {
  const baseStyle = {
    textDecoration: "none",
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.5 : 1,
    transition: "all 0.2s ease",
    ...style,
  };

  const variantStyles = {
    default: {
      color: "#FFFFFF", // White text for dark theme
      textDecoration: "none",
      "&:hover": {
        textDecoration: "underline",
      },
    },
    subtle: {
      color: tokens.color.neutral[200].value, // Lighter gray for better contrast
      textDecoration: "none",
      "&:hover": {
        color: "#FFFFFF", // White text for dark theme
      },
    },
    underline: {
      color: "#FFFFFF", // White text for dark theme
      textDecoration: "underline",
      "&:hover": {
        textDecoration: "none",
      },
    },
  };

  const colorStyles = {
    primary: { color: "#B8FF2A" }, // Slightly lighter version of brand-600 for better contrast
    secondary: { color: tokens.color.neutral[200].value }, // Lighter gray for better contrast
    success: { color: tokens.color.semantic.success.value },
    warning: { color: tokens.color.semantic.warning.value },
    error: { color: tokens.color.semantic.error.value },
  };

  const combinedStyle = {
    ...baseStyle,
    ...variantStyles[variant],
    ...colorStyles[color],
  };

  const handleClick = (e: React.MouseEvent) => {
    if (disabled) {
      e.preventDefault();
      return;
    }
    if (onClick) {
      e.preventDefault();
      onClick();
    }
  };

  const linkProps = {
    href: disabled ? undefined : href,
    onClick: handleClick,
    target: external ? "_blank" : undefined,
    rel: external ? "noopener noreferrer" : undefined,
    className,
    style: combinedStyle,
  };

  return <a {...linkProps}>{children}</a>;
};
