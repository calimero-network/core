import React from "react";

type LayoutProps = React.PropsWithChildren<{
  className?: string;
  style?: React.CSSProperties;
  maxWidth?: string | number;
  padding?: string | number;
  centered?: boolean;
  variant?: "auth" | "default";
}>;

export function Layout({
  children,
  className = "",
  style,
  maxWidth = "400px",
  padding = "24px",
  centered = true,
  variant = "auth",
}: LayoutProps) {
  const baseStyles: React.CSSProperties = {
    width: "100%",
    maxWidth: typeof maxWidth === "number" ? `${maxWidth}px` : maxWidth,
    padding: typeof padding === "number" ? `${padding}px` : padding,
    fontFamily: "var(--font-body)",
    ...(centered && {
      margin: "0 auto",
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      justifyContent: "center",
      minHeight: variant === "auth" ? "100vh" : "auto",
    }),
    ...style,
  };

  return (
    <div className={className} style={baseStyles}>
      {children}
    </div>
  );
}

// Auth-specific layout with additional styling
export function AuthLayout({
  children,
  className = "",
  style,
  maxWidth = "400px",
  padding = "32px",
  ...props
}: Omit<LayoutProps, "variant" | "centered">) {
  const authStyles: React.CSSProperties = {
    background: "var(--color-background-primary)",
    borderRadius: "var(--radius-lg)",
    border: "1px solid var(--color-neutral-600)",
    boxShadow: "0 8px 32px rgba(0, 0, 0, 0.3)",
    ...style,
  };

  return (
    <Layout
      {...props}
      className={className}
      style={authStyles}
      maxWidth={maxWidth}
      padding={padding}
      centered={true}
      variant="auth"
    >
      {children}
    </Layout>
  );
}

// Container layout for general use
export function Container({
  children,
  className = "",
  style,
  maxWidth = "1200px",
  padding = "16px",
  ...props
}: Omit<LayoutProps, "variant" | "centered">) {
  return (
    <Layout
      {...props}
      className={className}
      style={style}
      maxWidth={maxWidth}
      padding={padding}
      centered={true}
      variant="default"
    >
      {children}
    </Layout>
  );
}
