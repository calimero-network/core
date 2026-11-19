import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";
import { Text } from "./Typography";
import { Button } from "./Button";

export interface FilePreviewProps {
  file: File;
  onRemove?: () => void;
  showSize?: boolean;
  variant?: "compact" | "detailed";
  size?: "sm" | "md" | "lg";
  className?: string;
  style?: React.CSSProperties;
}

export function FilePreview({
  file,
  onRemove,
  showSize = true,
  variant = "compact",
  size = "md",
  className = "",
  style = {},
}: FilePreviewProps) {
  const sizeStyles = {
    sm: {
      padding: "8px",
      iconSize: "sm" as const,
      fontSize: "12px",
      gap: "8px",
    },
    md: {
      padding: "12px",
      iconSize: "md" as const,
      fontSize: "14px",
      gap: "12px",
    },
    lg: {
      padding: "16px",
      iconSize: "lg" as const,
      fontSize: "16px",
      gap: "16px",
    },
  };

  const getFileIcon = (file: File) => {
    const type = file.type.toLowerCase();

    if (type.startsWith("image/")) return "image";
    if (type.startsWith("video/")) return "video";
    if (type.startsWith("audio/")) return "music";
    if (type.includes("pdf")) return "file-text";
    if (type.includes("word") || type.includes("document")) return "file-text";
    if (type.includes("excel") || type.includes("spreadsheet")) return "table";
    if (type.includes("powerpoint") || type.includes("presentation"))
      return "presentation";
    if (type.includes("zip") || type.includes("archive")) return "archive";
    return "file";
  };

  const formatFileSize = (bytes: number) => {
    if (bytes === 0) return "0 Bytes";
    const k = 1024;
    const sizes = ["Bytes", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
  };

  const currentSize = sizeStyles[size];

  const containerStyles: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: currentSize.gap,
    padding: currentSize.padding,
    background: "var(--color-background-primary)",
    border: `1px solid ${tokens.color.neutral[700].value}`,
    borderRadius: "8px",
    ...style,
  };

  const compactStyles: React.CSSProperties =
    variant === "compact"
      ? {
          padding: "6px 8px",
          gap: "8px",
        }
      : {};

  const detailedStyles: React.CSSProperties =
    variant === "detailed"
      ? {
          flexDirection: "column",
          alignItems: "flex-start",
          textAlign: "left",
        }
      : {};

  return (
    <div
      className={className}
      style={{
        ...containerStyles,
        ...compactStyles,
        ...detailedStyles,
      }}
    >
      <div
        style={{ display: "flex", alignItems: "center", gap: currentSize.gap }}
      >
        <Icon
          name={getFileIcon(file)}
          size={currentSize.iconSize}
          color="muted"
        />
        <div style={{ minWidth: 0, flex: 1 }}>
          <Text
            size="sm"
            color="primary"
            style={{
              margin: 0,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {file.name}
          </Text>
          {showSize && (
            <Text size="sm" color="muted" style={{ margin: 0 }}>
              {formatFileSize(file.size)}
            </Text>
          )}
        </div>
      </div>

      {onRemove && (
        <Button
          variant="secondary"
          onClick={onRemove}
          style={{
            padding: "4px",
            minWidth: "auto",
            height: "auto",
          }}
          aria-label={`Remove ${file.name}`}
        >
          <Icon name="x" size="xs" />
        </Button>
      )}
    </div>
  );
}
