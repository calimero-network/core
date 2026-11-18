import React, { useState, useRef, useEffect } from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface TooltipProps {
  children: React.ReactNode;
  content: React.ReactNode;
  placement?: "top" | "bottom" | "left" | "right";
  trigger?: "hover" | "click" | "focus";
  delay?: number;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export const Tooltip: React.FC<TooltipProps> = ({
  children,
  content,
  placement = "top",
  trigger = "hover",
  delay = 200,
  disabled = false,
  className = "",
  style = {},
}) => {
  const [isVisible, setIsVisible] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const triggerRef = useRef<HTMLDivElement>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const timeoutRef = useRef<number | undefined>(undefined);

  const updatePosition = () => {
    if (!triggerRef.current || !tooltipRef.current) return;

    const triggerRect = triggerRef.current.getBoundingClientRect();
    const tooltipRect = tooltipRef.current.getBoundingClientRect();
    const scrollTop = window.pageYOffset || document.documentElement.scrollTop;
    const scrollLeft =
      window.pageXOffset || document.documentElement.scrollLeft;

    let top = 0;
    let left = 0;

    switch (placement) {
      case "top":
        top = triggerRect.top + scrollTop - tooltipRect.height - 8;
        left =
          triggerRect.left +
          scrollLeft +
          (triggerRect.width - tooltipRect.width) / 2;
        break;
      case "bottom":
        top = triggerRect.bottom + scrollTop + 8;
        left =
          triggerRect.left +
          scrollLeft +
          (triggerRect.width - tooltipRect.width) / 2;
        break;
      case "left":
        top =
          triggerRect.top +
          scrollTop +
          (triggerRect.height - tooltipRect.height) / 2;
        left = triggerRect.left + scrollLeft - tooltipRect.width - 8;
        break;
      case "right":
        top =
          triggerRect.top +
          scrollTop +
          (triggerRect.height - tooltipRect.height) / 2;
        left = triggerRect.right + scrollLeft + 8;
        break;
    }

    setPosition({ top, left });
  };

  const showTooltip = () => {
    if (disabled) return;

    if (timeoutRef.current) {
      window.clearTimeout(timeoutRef.current);
    }

    timeoutRef.current = window.setTimeout(() => {
      setIsVisible(true);
      setTimeout(updatePosition, 0);
    }, delay);
  };

  const hideTooltip = () => {
    if (timeoutRef.current) {
      window.clearTimeout(timeoutRef.current);
    }
    setIsVisible(false);
  };

  const handleMouseEnter = () => {
    if (trigger === "hover") {
      showTooltip();
    }
  };

  const handleMouseLeave = () => {
    if (trigger === "hover") {
      hideTooltip();
    }
  };

  const handleClick = () => {
    if (trigger === "click") {
      if (isVisible) {
        hideTooltip();
      } else {
        showTooltip();
      }
    }
  };

  const handleFocus = () => {
    if (trigger === "focus") {
      showTooltip();
    }
  };

  const handleBlur = () => {
    if (trigger === "focus") {
      hideTooltip();
    }
  };

  useEffect(() => {
    if (isVisible) {
      updatePosition();
      const handleScroll = () => updatePosition();
      const handleResize = () => updatePosition();

      window.addEventListener("scroll", handleScroll);
      window.addEventListener("resize", handleResize);

      return () => {
        window.removeEventListener("scroll", handleScroll);
        window.removeEventListener("resize", handleResize);
      };
    }
  }, [isVisible, placement]);

  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, []);

  const tooltipStyle: React.CSSProperties = {
    position: "absolute",
    top: position.top,
    left: position.left,
    zIndex: 1000,
    padding: "8px 12px",
    backgroundColor: tokens.color.neutral[900].value,
    color: tokens.color.neutral[200].value,
    fontSize: "14px",
    borderRadius: tokens.radius.sm.value,
    boxShadow:
      "0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06)",
    whiteSpace: "nowrap" as const,
    opacity: isVisible ? 1 : 0,
    visibility: isVisible ? ("visible" as const) : ("hidden" as const),
    transition: "opacity 0.2s ease, visibility 0.2s ease",
    ...style,
  };

  const arrowStyle = {
    position: "absolute" as const,
    width: 0,
    height: 0,
    border: "4px solid transparent",
  };

  const getArrowStyle = () => {
    switch (placement) {
      case "top":
        return {
          ...arrowStyle,
          top: "100%",
          left: "50%",
          transform: "translateX(-50%)",
          borderTopColor: tokens.color.neutral[900].value,
        };
      case "bottom":
        return {
          ...arrowStyle,
          bottom: "100%",
          left: "50%",
          transform: "translateX(-50%)",
          borderBottomColor: tokens.color.neutral[900].value,
        };
      case "left":
        return {
          ...arrowStyle,
          left: "100%",
          top: "50%",
          transform: "translateY(-50%)",
          borderLeftColor: tokens.color.neutral[900].value,
        };
      case "right":
        return {
          ...arrowStyle,
          right: "100%",
          top: "50%",
          transform: "translateY(-50%)",
          borderRightColor: tokens.color.neutral[900].value,
        };
    }
  };

  return (
    <>
      <div
        ref={triggerRef}
        onMouseEnter={handleMouseEnter}
        onMouseLeave={handleMouseLeave}
        onClick={handleClick}
        onFocus={handleFocus}
        onBlur={handleBlur}
        style={{ display: "inline-block" }}
      >
        {children}
      </div>

      {isVisible && (
        <div ref={tooltipRef} className={className} style={tooltipStyle}>
          {content}
          <div style={getArrowStyle()} />
        </div>
      )}
    </>
  );
};
