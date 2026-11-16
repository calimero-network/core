import React, { useState, useRef, useEffect } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Button } from "./Button";
import { Icon } from "./Icon";

export interface CopyToClipboardProps {
  text: string;
  children?: React.ReactNode;
  onCopy?: (text: string) => void;
  onError?: (error: Error) => void;
  successMessage?: string;
  errorMessage?: string;
  showToast?: boolean;
  variant?: "button" | "icon" | "text" | "custom";
  size?: "small" | "medium" | "large";
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  copyIcon?: React.ReactNode;
  successIcon?: React.ReactNode;
  errorIcon?: React.ReactNode;
  showFeedback?: boolean;
  feedbackDuration?: number;
}

export function CopyToClipboard({
  text,
  children,
  onCopy,
  onError,
  successMessage = "Copied to clipboard!",
  errorMessage = "Failed to copy to clipboard",
  showToast = true,
  variant = "button",
  size = "medium",
  disabled = false,
  className = "",
  style = {},
  copyIcon,
  successIcon,
  errorIcon,
  showFeedback = true,
  feedbackDuration = 2000,
}: CopyToClipboardProps) {
  const [status, setStatus] = useState<"idle" | "success" | "error">("idle");
  const [toasts, setToasts] = useState<
    Array<{ id: string; message: string; type: "success" | "error" }>
  >([]);
  const timeoutRef = useRef<number | undefined>(undefined);

  // Clear status after duration
  useEffect(() => {
    if (status !== "idle") {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }

      timeoutRef.current = setTimeout(() => {
        setStatus("idle");
      }, feedbackDuration);
    }

    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, [status, feedbackDuration]);

  // Copy to clipboard
  const copyToClipboard = async () => {
    if (disabled) return;

    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(text);
      } else {
        // Fallback for older browsers
        const textArea = document.createElement("textarea");
        textArea.value = text;
        textArea.style.position = "fixed";
        textArea.style.left = "-999999px";
        textArea.style.top = "-999999px";
        document.body.appendChild(textArea);
        textArea.focus();
        textArea.select();
        document.execCommand("copy");
        textArea.remove();
      }

      setStatus("success");
      onCopy?.(text);

      if (showToast) {
        const toastId = Date.now().toString();
        setToasts((prev) => [
          ...prev,
          { id: toastId, message: successMessage, type: "success" },
        ]);

        setTimeout(() => {
          setToasts((prev) => prev.filter((toast) => toast.id !== toastId));
        }, 3000);
      }
    } catch (error) {
      setStatus("error");
      onError?.(error as Error);

      if (showToast) {
        const toastId = Date.now().toString();
        setToasts((prev) => [
          ...prev,
          { id: toastId, message: errorMessage, type: "error" },
        ]);

        setTimeout(() => {
          setToasts((prev) => prev.filter((toast) => toast.id !== toastId));
        }, 3000);
      }
    }
  };

  // Get icon based on status
  const getIcon = () => {
    if (status === "success") {
      return successIcon || <Icon name="check" size="sm" />;
    }
    if (status === "error") {
      return errorIcon || <Icon name="x" size="sm" />;
    }
    return copyIcon || <Icon name="copy" size="sm" />;
  };

  // Get button text based on status
  const getButtonText = () => {
    if (status === "success") {
      return "Copied!";
    }
    if (status === "error") {
      return "Error";
    }
    return "Copy";
  };

  // Get color based on status
  const getColor = () => {
    if (status === "success") {
      return tokens.color.semantic.success.value;
    }
    if (status === "error") {
      return tokens.color.semantic.error.value;
    }
    return undefined;
  };

  const sizeStyles = {
    small: { padding: "4px 8px", fontSize: "12px", iconSize: 14 },
    medium: { padding: "8px 12px", fontSize: "14px", iconSize: 16 },
    large: { padding: "12px 16px", fontSize: "16px", iconSize: 18 },
  };

  const currentSizeStyle = sizeStyles[size];

  // Render based on variant
  const renderContent = () => {
    switch (variant) {
      case "icon":
        return (
          <button
            type="button"
            onClick={copyToClipboard}
            disabled={disabled}
            className={className}
            style={{
              background: "transparent",
              border: "none",
              cursor: disabled ? "not-allowed" : "pointer",
              color: getColor() || tokens.color.neutral[400].value,
              padding: "8px",
              borderRadius: "4px",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              transition: "all 0.2s ease",
              opacity: disabled ? 0.5 : 1,
              ...style,
            }}
            onMouseEnter={(e) => {
              if (!disabled) {
                e.currentTarget.style.background =
                  "var(--color-background-secondary)";
                e.currentTarget.style.color =
                  getColor() || tokens.color.neutral[300].value;
              }
            }}
            onMouseLeave={(e) => {
              if (!disabled) {
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.color =
                  getColor() || tokens.color.neutral[400].value;
              }
            }}
          >
            {getIcon()}
          </button>
        );

      case "text":
        return (
          <button
            type="button"
            onClick={copyToClipboard}
            disabled={disabled}
            className={className}
            style={{
              background: "transparent",
              border: "none",
              cursor: disabled ? "not-allowed" : "pointer",
              color: getColor() || tokens.color.brand[600].value,
              padding: "4px 0",
              fontSize: currentSizeStyle.fontSize,
              textDecoration: "underline",
              textDecorationColor: "transparent",
              transition: "all 0.2s ease",
              opacity: disabled ? 0.5 : 1,
              ...style,
            }}
            onMouseEnter={(e) => {
              if (!disabled) {
                e.currentTarget.style.textDecorationColor =
                  getColor() || tokens.color.brand[600].value;
              }
            }}
            onMouseLeave={(e) => {
              if (!disabled) {
                e.currentTarget.style.textDecorationColor = "transparent";
              }
            }}
          >
            {getButtonText()}
          </button>
        );

      case "custom":
        return (
          <div
            onClick={copyToClipboard}
            style={{
              cursor: disabled ? "not-allowed" : "pointer",
              opacity: disabled ? 0.5 : 1,
              ...style,
            }}
            className={className}
          >
            {children}
          </div>
        );

      case "button":
      default:
        return (
          <Button
            onClick={copyToClipboard}
            disabled={disabled}
            variant={
              status === "success"
                ? "success"
                : status === "error"
                  ? "error"
                  : "primary"
            }
            className={className}
            style={style}
          >
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              {getIcon()}
              {getButtonText()}
            </div>
          </Button>
        );
    }
  };

  return (
    <>
      {renderContent()}

      {/* Toast notifications */}
      {showToast && toasts.length > 0 && (
        <div
          style={{
            position: "fixed",
            top: "20px",
            right: "20px",
            zIndex: 1000,
            display: "flex",
            flexDirection: "column",
            gap: "8px",
          }}
        >
          {toasts.map((toast) => (
            <div
              key={toast.id}
              style={{
                padding: "12px 16px",
                background:
                  toast.type === "success"
                    ? "var(--color-semantic-success)"
                    : "var(--color-semantic-error)",
                color: "#FFFFFF",
                borderRadius: "6px",
                boxShadow: "0 4px 12px rgba(0, 0, 0, 0.3)",
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                minWidth: "200px",
              }}
            >
              <span>{toast.message}</span>
              <button
                onClick={() =>
                  setToasts((prev) => prev.filter((t) => t.id !== toast.id))
                }
                style={{
                  background: "none",
                  border: "none",
                  color: "#FFFFFF",
                  cursor: "pointer",
                  padding: "4px",
                  marginLeft: "8px",
                }}
              >
                <Icon name="x" size="sm" />
              </button>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

// Hook for copy to clipboard functionality
export function useCopyToClipboard() {
  const [status, setStatus] = useState<"idle" | "success" | "error">("idle");

  const copy = async (text: string) => {
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(text);
      } else {
        // Fallback for older browsers
        const textArea = document.createElement("textarea");
        textArea.value = text;
        textArea.style.position = "fixed";
        textArea.style.left = "-999999px";
        textArea.style.top = "-999999px";
        document.body.appendChild(textArea);
        textArea.focus();
        textArea.select();
        document.execCommand("copy");
        textArea.remove();
      }

      setStatus("success");
      setTimeout(() => setStatus("idle"), 2000);
      return true;
    } catch (error) {
      setStatus("error");
      setTimeout(() => setStatus("idle"), 2000);
      return false;
    }
  };

  return { copy, status };
}
