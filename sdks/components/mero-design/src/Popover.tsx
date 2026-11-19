import React, { useState, useRef, useEffect, useCallback } from "react";
import { tokens } from "@calimero-network/mero-tokens";

interface PopoverProps {
  children: React.ReactNode;
  content: React.ReactNode;
  isOpen?: boolean;
  onOpenChange?: (isOpen: boolean) => void;
  placement?:
    | "top"
    | "bottom"
    | "left"
    | "right"
    | "top-start"
    | "top-end"
    | "bottom-start"
    | "bottom-end"
    | "left-start"
    | "left-end"
    | "right-start"
    | "right-end";
  trigger?: "click" | "hover" | "focus";
  disabled?: boolean;
  closeOnClickOutside?: boolean;
  closeOnEscape?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export const Popover: React.FC<PopoverProps> = ({
  children,
  content,
  isOpen: controlledIsOpen,
  onOpenChange,
  placement = "bottom",
  trigger = "click",
  disabled = false,
  closeOnClickOutside = true,
  closeOnEscape = true,
  className = "",
  style = {},
}) => {
  const [internalIsOpen, setInternalIsOpen] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const triggerRef = useRef<HTMLDivElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const timeoutRef = useRef<number | undefined>(undefined);

  const isOpen =
    controlledIsOpen !== undefined ? controlledIsOpen : internalIsOpen;

  const updatePosition = useCallback(() => {
    if (!triggerRef.current || !popoverRef.current) return;

    const triggerRect = triggerRef.current.getBoundingClientRect();
    const popoverRect = popoverRef.current.getBoundingClientRect();
    const scrollTop = window.pageYOffset || document.documentElement.scrollTop;
    const scrollLeft =
      window.pageXOffset || document.documentElement.scrollLeft;

    let top = 0;
    let left = 0;

    switch (placement) {
      case "top":
        top = triggerRect.top + scrollTop - popoverRect.height - 8;
        left =
          triggerRect.left +
          scrollLeft +
          (triggerRect.width - popoverRect.width) / 2;
        break;
      case "top-start":
        top = triggerRect.top + scrollTop - popoverRect.height - 8;
        left = triggerRect.left + scrollLeft;
        break;
      case "top-end":
        top = triggerRect.top + scrollTop - popoverRect.height - 8;
        left = triggerRect.right + scrollLeft - popoverRect.width;
        break;
      case "bottom":
        top = triggerRect.bottom + scrollTop + 8;
        left =
          triggerRect.left +
          scrollLeft +
          (triggerRect.width - popoverRect.width) / 2;
        break;
      case "bottom-start":
        top = triggerRect.bottom + scrollTop + 8;
        left = triggerRect.left + scrollLeft;
        break;
      case "bottom-end":
        top = triggerRect.bottom + scrollTop + 8;
        left = triggerRect.right + scrollLeft - popoverRect.width;
        break;
      case "left":
        top =
          triggerRect.top +
          scrollTop +
          (triggerRect.height - popoverRect.height) / 2;
        left = triggerRect.left + scrollLeft - popoverRect.width - 8;
        break;
      case "left-start":
        top = triggerRect.top + scrollTop;
        left = triggerRect.left + scrollLeft - popoverRect.width - 8;
        break;
      case "left-end":
        top = triggerRect.bottom + scrollTop - popoverRect.height;
        left = triggerRect.left + scrollLeft - popoverRect.width - 8;
        break;
      case "right":
        top =
          triggerRect.top +
          scrollTop +
          (triggerRect.height - popoverRect.height) / 2;
        left = triggerRect.right + scrollLeft + 8;
        break;
      case "right-start":
        top = triggerRect.top + scrollTop;
        left = triggerRect.right + scrollLeft + 8;
        break;
      case "right-end":
        top = triggerRect.bottom + scrollTop - popoverRect.height;
        left = triggerRect.right + scrollLeft + 8;
        break;
    }

    setPosition({ top, left });
  }, [placement]);

  const openPopover = useCallback(() => {
    if (disabled) return;
    if (onOpenChange) {
      onOpenChange(true);
    } else {
      setInternalIsOpen(true);
    }
    setTimeout(updatePosition, 0);
  }, [disabled, onOpenChange, updatePosition]);

  const closePopover = useCallback(() => {
    if (onOpenChange) {
      onOpenChange(false);
    } else {
      setInternalIsOpen(false);
    }
  }, [onOpenChange]);

  const handleTriggerClick = () => {
    if (trigger === "click") {
      if (isOpen) {
        closePopover();
      } else {
        openPopover();
      }
    }
  };

  const handleTriggerMouseEnter = () => {
    if (trigger === "hover") {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
      openPopover();
    }
  };

  const handleTriggerMouseLeave = () => {
    if (trigger === "hover") {
      timeoutRef.current = window.setTimeout(() => {
        closePopover();
      }, 100);
    }
  };

  const handlePopoverMouseEnter = () => {
    if (trigger === "hover" && timeoutRef.current) {
      clearTimeout(timeoutRef.current);
    }
  };

  const handlePopoverMouseLeave = () => {
    if (trigger === "hover") {
      timeoutRef.current = window.setTimeout(() => {
        closePopover();
      }, 100);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" && closeOnEscape) {
      closePopover();
    }
  };

  useEffect(() => {
    if (isOpen) {
      updatePosition();
    }
  }, [isOpen, updatePosition]);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        closeOnClickOutside &&
        isOpen &&
        triggerRef.current &&
        popoverRef.current &&
        !triggerRef.current.contains(event.target as Node) &&
        !popoverRef.current.contains(event.target as Node)
      ) {
        closePopover();
      }
    };

    if (isOpen) {
      document.addEventListener("mousedown", handleClickOutside);
      return () =>
        document.removeEventListener("mousedown", handleClickOutside);
    }
  }, [isOpen, closeOnClickOutside, closePopover]);

  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, []);

  const popoverStyle: React.CSSProperties = {
    position: "absolute",
    top: position.top,
    left: position.left,
    zIndex: 1000,
    backgroundColor: tokens.color.background.primary.value,
    border: `1px solid ${tokens.color.neutral[600].value}`,
    borderRadius: tokens.radius.md.value,
    boxShadow:
      "0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05)",
    padding: tokens.space["3"].value,
    maxWidth: "300px",
    opacity: isOpen ? 1 : 0,
    visibility: isOpen ? "visible" : "hidden",
    transition: "opacity 0.2s ease, visibility 0.2s ease",
    ...style,
  };

  return (
    <div
      className={className}
      style={{ position: "relative", display: "inline-block" }}
    >
      <div
        ref={triggerRef}
        onClick={handleTriggerClick}
        onMouseEnter={handleTriggerMouseEnter}
        onMouseLeave={handleTriggerMouseLeave}
        onFocus={openPopover}
        onBlur={closePopover}
        onKeyDown={handleKeyDown}
        style={{ display: "inline-block" }}
      >
        {children}
      </div>
      {isOpen && (
        <div
          ref={popoverRef}
          style={popoverStyle}
          onMouseEnter={handlePopoverMouseEnter}
          onMouseLeave={handlePopoverMouseLeave}
        >
          {content}
        </div>
      )}
    </div>
  );
};
