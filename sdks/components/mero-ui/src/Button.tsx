import React from "react";

/**
 * Button size variants
 * @public
 */
export type ButtonSize = "sm" | "md" | "lg" | "xl";

/**
 * Button style variants
 * @public
 */
export type ButtonVariant =
  | "primary"        // Brand color with black text (default)
  | "secondary"      // Dark background with white text and border
  | "success"        // Green semantic color
  | "warning"        // Orange semantic color
  | "error"          // Red semantic color (same as destructive)
  | "info"           // Blue semantic color
  | "outline"        // Transparent with border
  | "ghost"          // Transparent without border
  | "destructive";   // Red semantic color (same as error)

/**
 * Button component props
 * @public
 */
export interface ButtonProps extends React.PropsWithChildren {
  /** Render as a different element or component (polymorphic) */
  as?: React.ElementType;
  /** Click handler function */
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  /** Whether the button is disabled */
  disabled?: boolean;
  /** Additional CSS class name */
  className?: string;
  /** Inline styles */
  style?: React.CSSProperties;
  /** HTML button type (only applies when as="button") */
  type?: "button" | "submit" | "reset";
  /** Visual variant/style of the button */
  variant?: ButtonVariant;
  /** Size of the button */
  size?: ButtonSize;
  /** Icon to display on the left side of the button text */
  leftIcon?: React.ReactNode;
  /** Icon to display on the right side of the button text */
  rightIcon?: React.ReactNode;
  /** Make button span full width of container */
  fullWidth?: boolean;
  /** Whether button has rounded corners (default: true, 12px radius). When false, uses 0px (sharp corners) */
  rounded?: boolean;
  /** Accessibility: aria-label for screen readers */
  "aria-label"?: string;
  /** Accessibility: aria-current attribute */
  "aria-current"?:
    | "page"
    | "step"
    | "location"
    | "date"
    | "time"
    | "true"
    | "false";
  /** HTML title attribute for tooltip */
  title?: string;
}

/**
 * Button Component
 * 
 * A versatile, accessible button component with multiple variants, sizes, and states.
 * Supports icons, full-width layout, and border radius control.
 * For loading states, compose with <Spinner /> in children and use disabled + aria-busy.
 * 
 * @example
 * ```tsx
 * import { Button, Spinner } from '@calimero-network/mero-ui';
 * 
 * // Basic usage
 * <Button onClick={() => console.log('clicked')}>
 *   Click me
 * </Button>
 * 
 * // With variant and size
 * <Button variant="secondary" size="lg">
 *   Large Secondary Button
 * </Button>
 * 
 * // With loading state (composed pattern)
 * <Button disabled aria-busy>
 *   <Spinner size="xs" color="current" thickness="thin" style={{ marginRight: 8 }} />
 *   Processing...
 * </Button>
 * 
 * // With icons
 * <Button leftIcon={<Icon />} rightIcon={<Arrow />}>
 *   With Icons
 * </Button>
 * 
 * // Full width
 * <Button fullWidth>Full Width Button</Button>
 * 
 * // Sharp corners (not rounded)
 * <Button rounded={false}>Sharp Button</Button>
 * ```
 * 
 * @public
 */

export const Button = React.forwardRef<
  HTMLElement,
  ButtonProps
>(
  (
    {
      as,
      children,
      onClick,
      disabled = false,
      className = "",
      style,
      type = "button",
      variant = "primary",
      size = "md",
      leftIcon,
      rightIcon,
      fullWidth = false,
      rounded = true,
      "aria-label": ariaLabel,
      "aria-current": ariaCurrent,
      title,
      ...restProps
    },
    ref
  ) => {
    const baseHeight: Record<ButtonSize, number> = { sm: 32, md: 40, lg: 48, xl: 56 };
    const basePaddingX: Record<ButtonSize, number> = { sm: 12, md: 16, lg: 20, xl: 24 };
    const baseFontSize: Record<ButtonSize, number> = { sm: 12, md: 14, lg: 16, xl: 18 };

    const baseStyles: React.CSSProperties = {
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      gap: 0, // Icons have their own margins, so gap is 0
      height: `${baseHeight[size]}px`,
      padding: `0 ${basePaddingX[size]}px`,
      borderRadius: rounded ? "12px" : "0px",
      border: "1px solid transparent",
      fontFamily: "var(--font-body)",
      fontSize: `${baseFontSize[size]}px`,
      fontWeight: 700,
      cursor: disabled ? "not-allowed" : "pointer",
      userSelect: "none",
      transition:
        "background-color 120ms ease, border-color 120ms ease, transform 60ms ease, box-shadow 120ms ease",
      width: fullWidth ? "100%" : undefined,
    };

    const [isActive, setIsActive] = React.useState(false);
    const [isHover, setIsHover] = React.useState(false);

    const isPrimary = variant === "primary";
    const isSecondary = variant === "secondary";

    const palette = (() => {
      switch (variant) {
        case "secondary":
          return {
            base: "#1A1A1A",
            hover: "#2A2A2A",
            active: "#111111",
            disabled: "#0F0F0F",
            text: "#FFFFFF",
            border: "#404040",
            borderHover: "#505050",
            borderActive: "#5A5A5A",
          };
        case "outline":
          return {
            base: "transparent",
            hover: "rgba(255,255,255,0.04)",
            active: "rgba(255,255,255,0.08)",
            disabled: "transparent",
            text: "#FFFFFF",
            border: "#404040",
            borderHover: "#505050",
            borderActive: "#5A5A5A",
          };
        case "ghost":
          return {
            base: "transparent",
            hover: "rgba(255,255,255,0.04)",
            active: "rgba(255,255,255,0.08)",
            disabled: "transparent",
            text: "#FFFFFF",
            border: "transparent",
            borderHover: "transparent",
            borderActive: "transparent",
          };
        case "destructive":
        case "error":
          return {
            base: "var(--color-semantic-error)",
            hover: "var(--color-semantic-error)",
            active: "var(--color-semantic-error)",
            disabled: "var(--color-semantic-error)",
            text: "#FFFFFF",
            border: "transparent",
            borderHover: "transparent",
            borderActive: "transparent",
          };
        case "success":
          return {
            base: "var(--color-semantic-success)",
            hover: "var(--color-semantic-success)",
            active: "var(--color-semantic-success)",
            disabled: "var(--color-semantic-success)",
            text: "#FFFFFF",
            border: "transparent",
            borderHover: "transparent",
            borderActive: "transparent",
          };
        case "warning":
          return {
            base: "var(--color-semantic-warning)",
            hover: "var(--color-semantic-warning)",
            active: "var(--color-semantic-warning)",
            disabled: "var(--color-semantic-warning)",
            text: "#000000",
            border: "transparent",
            borderHover: "transparent",
            borderActive: "transparent",
          };
        case "info":
          return {
            base: "var(--color-semantic-info)",
            hover: "var(--color-semantic-info)",
            active: "var(--color-semantic-info)",
            disabled: "var(--color-semantic-info)",
            text: "#FFFFFF",
            border: "transparent",
            borderHover: "transparent",
            borderActive: "transparent",
          };
        default:
          return {
            base: "var(--color-brand-600)",
            hover: "var(--color-brand-100)",
            active: "var(--color-brand-800)",
            disabled: "var(--color-brand-900)",
            text: "#000000",
            border: "transparent",
            borderHover: "transparent",
            borderActive: "transparent",
          };
      }
    })();

    let dynamicBackground = palette.base as string;
    let dynamicBorder = palette.border as string;
    let dynamicTextColor = palette.text as string;
    if (!disabled) {
      if (isActive) {
        dynamicBackground = palette.active as string;
        dynamicBorder = palette.borderActive as string;
      } else if (isHover) {
        dynamicBackground = palette.hover as string;
        dynamicBorder = palette.borderHover as string;
      }
    } else {
      dynamicBackground = palette.disabled as string;
    }

    const Component = as || "button";
    const isButton = Component === "button";
    
    const componentProps: any = {
      ref, // Always forward ref for polymorphic support
      style: {
        ...baseStyles,
        backgroundColor: dynamicBackground,
        borderColor: isSecondary || variant === "outline" ? dynamicBorder : "transparent",
        color: isPrimary ? "#000000" : dynamicTextColor,
        boxShadow:
          variant === "primary" && isHover && !disabled
            ? "0 0 20px rgba(165, 255, 17, 0.25)"
            : undefined,
        ...style,
      },
      className,
      "aria-label": ariaLabel,
      "aria-current": ariaCurrent,
      title,
      ...restProps,
    };

    if (isButton) {
      componentProps.type = type;
      componentProps.disabled = disabled;
      componentProps.onClick = onClick;
    } else {
      // For non-button elements, handle disabled state via aria and styling
      componentProps.onClick = disabled ? undefined : onClick;
      if (disabled) {
        componentProps["aria-disabled"] = true;
        componentProps.style = {
          ...componentProps.style,
          pointerEvents: "none",
          opacity: 0.5,
        };
      }
    }

    // Add mouse event handlers for interactive elements (buttons and links)
    if (!disabled) {
      componentProps.onMouseDown = () => setIsActive(true);
      componentProps.onMouseUp = () => setIsActive(false);
      componentProps.onMouseLeave = () => {
        setIsActive(false);
        setIsHover(false);
      };
      componentProps.onMouseEnter = () => setIsHover(true);
    }

    return (
      <Component {...componentProps}>
        {leftIcon && <span style={{ marginRight: 8 }}>{leftIcon}</span>}
        {children}
        {rightIcon && <span style={{ marginLeft: 8 }}>{rightIcon}</span>}
      </Component>
    );
  }
);

Button.displayName = "Button";
