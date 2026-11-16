import React from "react";

type LoaderProps = {
  size?: "small" | "medium" | "large";
  variant?: "spinner" | "dots" | "pulse";
  color?: string;
  className?: string;
  style?: React.CSSProperties;
  fullScreen?: boolean;
  overlay?: boolean;
  message?: string;
};

export function Loader({
  size = "medium",
  variant = "spinner",
  color = "var(--color-brand-600)",
  className = "",
  style,
  fullScreen = false,
  overlay = false,
  message,
}: LoaderProps) {
  const getSizeStyles = () => {
    switch (size) {
      case "small":
        return { width: "16px", height: "16px" };
      case "large":
        return { width: "48px", height: "48px" };
      default: // medium
        return { width: "24px", height: "24px" };
    }
  };

  const sizeStyles = getSizeStyles();

  const renderSpinner = () => (
    <div
      style={{
        ...sizeStyles,
        border: `2px solid transparent`,
        borderTop: `2px solid ${color}`,
        borderRadius: "50%",
        animation: "spin 1s linear infinite",
        ...style,
      }}
      className={className}
    />
  );

  const renderDots = () => (
    <div
      style={{
        display: "flex",
        gap: "4px",
        alignItems: "center",
        ...style,
      }}
      className={className}
    >
      {[0, 1, 2].map((i) => (
        <div
          key={i}
          style={{
            width: "8px",
            height: "8px",
            backgroundColor: color,
            borderRadius: "50%",
            animation: `bounce 1.4s ease-in-out ${i * 0.16}s infinite both`,
          }}
        />
      ))}
    </div>
  );

  const renderPulse = () => (
    <div
      style={{
        ...sizeStyles,
        backgroundColor: color,
        borderRadius: "50%",
        animation: "pulse 1.5s ease-in-out infinite",
        ...style,
      }}
      className={className}
    />
  );

  const renderLoader = () => {
    switch (variant) {
      case "dots":
        return renderDots();
      case "pulse":
        return renderPulse();
      default: // spinner
        return renderSpinner();
    }
  };

  const containerStyles: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    gap: "16px",
    ...(fullScreen && {
      position: "fixed",
      top: 0,
      left: 0,
      right: 0,
      bottom: 0,
      backgroundColor: overlay ? "rgba(0, 0, 0, 0.5)" : "transparent",
      zIndex: 9999,
    }),
  };

  const messageStyles: React.CSSProperties = {
    fontSize: "14px",
    color: "var(--color-neutral-300)",
    fontFamily: "var(--font-body)",
    textAlign: "center",
  };

  return (
    <div style={containerStyles}>
      {renderLoader()}
      {message && <div style={messageStyles}>{message}</div>}
      <style>{`
        @keyframes spin {
          0% { transform: rotate(0deg); }
          100% { transform: rotate(360deg); }
        }
        
        @keyframes bounce {
          0%, 80%, 100% {
            transform: scale(0);
          }
          40% {
            transform: scale(1);
          }
        }
        
        @keyframes pulse {
          0% {
            transform: scale(0.95);
            opacity: 0.7;
          }
          70% {
            transform: scale(1);
            opacity: 1;
          }
          100% {
            transform: scale(0.95);
            opacity: 0.7;
          }
        }
      `}</style>
    </div>
  );
}

// Inline loader for smaller spaces
export function InlineLoader({
  size = "small",
  variant = "spinner",
  color = "var(--color-brand-600)",
  className = "",
  style,
}: Omit<LoaderProps, "fullScreen" | "overlay" | "message">) {
  return (
    <Loader
      size={size}
      variant={variant}
      color={color}
      className={className}
      style={style}
      fullScreen={false}
    />
  );
}

// Full screen loader with overlay
export function FullScreenLoader({
  message = "Loading...",
  variant = "spinner",
  color = "var(--color-brand-600)",
  className = "",
  style,
}: Omit<LoaderProps, "fullScreen" | "overlay" | "size">) {
  return (
    <Loader
      message={message}
      variant={variant}
      color={color}
      className={className}
      style={style}
      fullScreen={true}
      overlay={true}
      size="large"
    />
  );
}

// Page loader for route transitions
export function PageLoader({
  message = "Loading page...",
  variant = "spinner",
  color = "var(--color-brand-600)",
  className = "",
  style,
}: Omit<LoaderProps, "fullScreen" | "overlay" | "size">) {
  return (
    <Loader
      message={message}
      variant={variant}
      color={color}
      className={className}
      style={style}
      fullScreen={true}
      overlay={false}
      size="large"
    />
  );
}
