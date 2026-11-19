import React, { useState, useRef, useCallback } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Button } from "./Button";
import { Icon } from "./Icon";

export interface FileUploadProps {
  accept?: string;
  multiple?: boolean;
  maxSize?: number; // in bytes
  maxFiles?: number;
  onUpload?: (files: File[]) => void;
  onRemove?: (file: File) => void;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  variant?: "default" | "compact" | "minimal";
  showPreview?: boolean;
  showProgress?: boolean;
  placeholder?: string;
  error?: boolean;
  errorMessage?: string;
  label?: string;
  required?: boolean;
}

export function FileUpload({
  accept,
  multiple = false,
  maxSize = 10 * 1024 * 1024, // 10MB default
  maxFiles = 10,
  onUpload,
  onRemove,
  disabled = false,
  className = "",
  style = {},
  variant = "default",
  showPreview = true,
  showProgress = false,
  placeholder = "Click to upload or drag and drop",
  error = false,
  errorMessage,
  label,
  required = false,
}: FileUploadProps) {
  const [files, setFiles] = useState<File[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState<Record<string, number>>(
    {},
  );
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Format file size
  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return "0 Bytes";
    const k = 1024;
    const sizes = ["Bytes", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
  };

  // Validate file
  const validateFile = (file: File): string | null => {
    if (maxSize && file.size > maxSize) {
      return `File size must be less than ${formatFileSize(maxSize)}`;
    }

    if (accept) {
      const acceptedTypes = accept.split(",").map((type) => type.trim());
      const fileExtension = "." + file.name.split(".").pop()?.toLowerCase();
      const mimeType = file.type;

      const isValidType = acceptedTypes.some((type) => {
        if (type.startsWith(".")) {
          return fileExtension === type;
        }
        if (type.includes("*")) {
          const baseType = type.replace("*", "");
          return mimeType.startsWith(baseType);
        }
        return mimeType === type;
      });

      if (!isValidType) {
        return `File type not accepted. Accepted types: ${accept}`;
      }
    }

    return null;
  };

  // Handle file selection
  const handleFiles = useCallback(
    (selectedFiles: FileList | null) => {
      if (!selectedFiles) return;

      const fileArray = Array.from(selectedFiles);
      const validFiles: File[] = [];
      const errors: string[] = [];

      // Check max files limit
      if (files.length + fileArray.length > maxFiles) {
        errors.push(`Maximum ${maxFiles} files allowed`);
      }

      // Validate each file
      fileArray.forEach((file) => {
        const error = validateFile(file);
        if (error) {
          errors.push(`${file.name}: ${error}`);
        } else {
          validFiles.push(file);
        }
      });

      // Show errors if any
      if (errors.length > 0) {
        alert(errors.join("\n"));
      }

      // Add valid files
      if (validFiles.length > 0) {
        const newFiles = multiple ? [...files, ...validFiles] : validFiles;
        setFiles(newFiles);
        onUpload?.(newFiles);
      }
    },
    [files, maxFiles, maxSize, accept, multiple, onUpload],
  );

  // Handle drag events
  const handleDrag = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.type === "dragenter" || e.type === "dragover") {
      setDragActive(true);
    } else if (e.type === "dragleave") {
      setDragActive(false);
    }
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setDragActive(false);

      if (disabled) return;

      const droppedFiles = e.dataTransfer.files;
      handleFiles(droppedFiles);
    },
    [disabled, handleFiles],
  );

  // Handle file input change
  const handleFileInputChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      handleFiles(e.target.files);
      // Reset input value to allow selecting the same file again
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
    },
    [handleFiles],
  );

  // Remove file
  const removeFile = useCallback(
    (fileToRemove: File) => {
      const newFiles = files.filter((file) => file !== fileToRemove);
      setFiles(newFiles);
      onRemove?.(fileToRemove);
      onUpload?.(newFiles);
    },
    [files, onRemove, onUpload],
  );

  // Simulate upload progress
  const simulateUpload = useCallback(async (file: File) => {
    setUploading(true);
    setUploadProgress((prev) => ({ ...prev, [file.name]: 0 }));

    // Simulate progress
    for (let i = 0; i <= 100; i += 10) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      setUploadProgress((prev) => ({ ...prev, [file.name]: i }));
    }

    setUploading(false);
    setUploadProgress((prev) => {
      const newProgress = { ...prev };
      delete newProgress[file.name];
      return newProgress;
    });
  }, []);

  // Get file icon based on type
  const getFileIcon = (file: File): string => {
    const extension = file.name.split(".").pop()?.toLowerCase();

    switch (extension) {
      case "pdf":
        return "file-text";
      case "doc":
      case "docx":
        return "file-text";
      case "xls":
      case "xlsx":
        return "file-spreadsheet";
      case "ppt":
      case "pptx":
        return "file-presentation";
      case "jpg":
      case "jpeg":
      case "png":
      case "gif":
      case "svg":
        return "image";
      case "mp4":
      case "avi":
      case "mov":
        return "video";
      case "mp3":
      case "wav":
      case "flac":
        return "music";
      case "zip":
      case "rar":
      case "7z":
        return "archive";
      default:
        return "file";
    }
  };

  const variantStyles = {
    default: {
      minHeight: "120px",
      padding: "24px",
      border: `2px dashed ${dragActive ? tokens.color.brand[600].value : tokens.color.neutral[600].value}`,
      borderRadius: "8px",
      background: dragActive
        ? tokens.color.brand[600].value + "10"
        : "transparent",
    },
    compact: {
      minHeight: "60px",
      padding: "12px",
      border: `1px dashed ${dragActive ? tokens.color.brand[600].value : tokens.color.neutral[600].value}`,
      borderRadius: "4px",
      background: dragActive
        ? tokens.color.brand[600].value + "10"
        : "transparent",
    },
    minimal: {
      minHeight: "40px",
      padding: "8px",
      border: "none",
      borderRadius: "4px",
      background: "transparent",
    },
  };

  const currentVariantStyle = variantStyles[variant];

  return (
    <div className={className} style={style}>
      {label && (
        <label
          style={{
            display: "block",
            fontSize: "14px",
            fontWeight: 500,
            color: "#FFFFFF",
            marginBottom: "8px",
            fontFamily: "var(--font-body)",
          }}
        >
          {label}
          {required && (
            <span
              style={{
                color: tokens.color.semantic.error.value,
                marginLeft: "4px",
              }}
            >
              *
            </span>
          )}
        </label>
      )}

      {/* Upload Area */}
      <div
        style={{
          ...currentVariantStyle,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          textAlign: "center",
          cursor: disabled ? "not-allowed" : "pointer",
          opacity: disabled ? 0.6 : 1,
          transition: "all 0.2s ease",
        }}
        onDragEnter={handleDrag}
        onDragLeave={handleDrag}
        onDragOver={handleDrag}
        onDrop={handleDrop}
        onClick={() => !disabled && fileInputRef.current?.click()}
      >
        <input
          ref={fileInputRef}
          type="file"
          accept={accept}
          multiple={multiple}
          onChange={handleFileInputChange}
          style={{ display: "none" }}
          disabled={disabled}
        />

        {variant !== "minimal" && (
          <Icon
            name="upload"
            size={variant === "compact" ? "sm" : "md"}
            style={{
              color: dragActive
                ? tokens.color.brand[600].value
                : tokens.color.neutral[400].value,
              marginBottom: "8px",
            }}
          />
        )}

        <div
          style={{
            fontSize: variant === "compact" ? "12px" : "14px",
            color: dragActive
              ? tokens.color.brand[600].value
              : tokens.color.neutral[300].value,
            marginBottom: variant === "minimal" ? "0" : "4px",
          }}
        >
          {placeholder}
        </div>

        {variant !== "minimal" && (
          <div
            style={{
              fontSize: "12px",
              color: tokens.color.neutral[400].value,
            }}
          >
            {accept && `Accepted: ${accept}`}
            {maxSize && ` • Max size: ${formatFileSize(maxSize)}`}
            {maxFiles > 1 && ` • Max files: ${maxFiles}`}
          </div>
        )}

        {variant !== "minimal" && (
          <Button
            variant="secondary"
            style={{ marginTop: "12px" }}
            disabled={disabled}
          >
            Choose Files
          </Button>
        )}
      </div>

      {/* File List */}
      {files.length > 0 && showPreview && (
        <div style={{ marginTop: "16px" }}>
          {files.map((file, index) => (
            <div
              key={`${file.name}-${index}`}
              style={{
                display: "flex",
                alignItems: "center",
                padding: "12px",
                background: "var(--color-background-secondary)",
                border: `1px solid ${tokens.color.neutral[600].value}`,
                borderRadius: "6px",
                marginBottom: "8px",
              }}
            >
              <Icon
                name={getFileIcon(file)}
                size="sm"
                style={{
                  color: tokens.color.neutral[400].value,
                  marginRight: "12px",
                }}
              />

              <div style={{ flex: 1, minWidth: 0 }}>
                <div
                  style={{
                    fontSize: "14px",
                    color: "#FFFFFF",
                    fontWeight: 500,
                    marginBottom: "2px",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {file.name}
                </div>

                <div
                  style={{
                    fontSize: "12px",
                    color: tokens.color.neutral[400].value,
                  }}
                >
                  {formatFileSize(file.size)}
                </div>

                {showProgress &&
                  uploading &&
                  uploadProgress[file.name] !== undefined && (
                    <div
                      style={{
                        marginTop: "8px",
                        width: "100%",
                        height: "4px",
                        background: tokens.color.neutral[700].value,
                        borderRadius: "2px",
                        overflow: "hidden",
                      }}
                    >
                      <div
                        style={{
                          width: `${uploadProgress[file.name]}%`,
                          height: "100%",
                          background: tokens.color.brand[600].value,
                          transition: "width 0.3s ease",
                        }}
                      />
                    </div>
                  )}
              </div>

              <Button
                variant="secondary"
                onClick={() => removeFile(file)}
                disabled={uploading}
                style={{
                  padding: "4px",
                  color: tokens.color.semantic.error.value,
                }}
              >
                <Icon name="x" size="sm" />
              </Button>
            </div>
          ))}
        </div>
      )}

      {error && errorMessage && (
        <div
          style={{
            fontSize: "12px",
            color: tokens.color.semantic.error.value,
            marginTop: "8px",
          }}
        >
          {errorMessage}
        </div>
      )}
    </div>
  );
}
