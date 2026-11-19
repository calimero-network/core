import React, { useState } from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface ImageProps {
  src: string;
  alt: string;
  width?: number | string;
  height?: number | string;
  fit?: "cover" | "contain" | "fill" | "none" | "scale-down";
  radius?: "none" | "sm" | "md" | "lg" | "full";
  fallback?: React.ReactNode;
  loading?: "lazy" | "eager";
  className?: string;
  style?: React.CSSProperties;
  onLoad?: () => void;
  onError?: () => void;
}

export const Image: React.FC<ImageProps> = ({
  src,
  alt,
  width,
  height,
  fit = "cover",
  radius = "md",
  fallback,
  loading = "lazy",
  className = "",
  style = {},
  onLoad,
  onError,
}) => {
  const [isLoading, setIsLoading] = useState(true);
  const [hasError, setHasError] = useState(false);

  const radiusMap = {
    none: "0",
    sm: tokens.radius.sm.value,
    md: tokens.radius.md.value,
    lg: tokens.radius.lg.value,
    full: "50%",
  };

  const baseStyle = {
    width: width || "100%",
    height: height || "auto",
    borderRadius: radiusMap[radius],
    overflow: "hidden",
    backgroundColor: tokens.color.background.tertiary.value,
    position: "relative" as const,
    ...style,
  };

  const imageStyle = {
    width: "100%",
    height: "100%",
    objectFit: fit,
    transition: "opacity 0.3s ease",
    opacity: isLoading ? 0 : 1,
  };

  const loadingStyle = {
    position: "absolute" as const,
    top: "50%",
    left: "50%",
    transform: "translate(-50%, -50%)",
    color: tokens.color.neutral[300].value,
  };

  const errorStyle = {
    position: "absolute" as const,
    top: "50%",
    left: "50%",
    transform: "translate(-50%, -50%)",
    color: tokens.color.neutral[300].value,
    textAlign: "center" as const,
    padding: "16px",
  };

  const handleLoad = () => {
    setIsLoading(false);
    onLoad?.();
  };

  const handleError = () => {
    setIsLoading(false);
    setHasError(true);
    onError?.();
  };

  const renderFallback = () => {
    if (fallback) {
      return fallback;
    }

    return (
      <div style={errorStyle}>
        <svg
          width="48"
          height="48"
          viewBox="0 0 24 24"
          fill="none"
          style={{
            color: tokens.color.neutral[300].value,
            marginBottom: "8px",
          }}
        >
          <path
            d="M21 19V5c0-1.1-.9-2-2-2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2zM8.5 13.5l2.5 3.01L14.5 12l4.5 6H5l3.5-4.5z"
            fill="currentColor"
          />
        </svg>
        <div>Failed to load image</div>
      </div>
    );
  };

  return (
    <div className={className} style={baseStyle}>
      {isLoading && (
        <div style={loadingStyle}>
          <svg
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            style={{ animation: "spin 1s linear infinite" }}
          >
            <circle
              cx="12"
              cy="12"
              r="10"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeDasharray="60 40"
              fill="none"
            />
          </svg>
        </div>
      )}

      {hasError ? (
        renderFallback()
      ) : (
        <img
          src={src}
          alt={alt}
          loading={loading}
          style={imageStyle}
          onLoad={handleLoad}
          onError={handleError}
        />
      )}
    </div>
  );
};
