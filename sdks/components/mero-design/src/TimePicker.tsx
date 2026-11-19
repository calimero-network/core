import React, { useState, useRef, useEffect } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Input } from "./Input";
import { Button } from "./Button";
import { Icon } from "./Icon";

export interface TimePickerProps {
  value?: Date | null;
  onChange?: (date: Date | null) => void;
  placeholder?: string;
  disabled?: boolean;
  format?: "12h" | "24h";
  minTime?: string; // HH:mm format
  maxTime?: string; // HH:mm format
  step?: number; // minutes
  className?: string;
  style?: React.CSSProperties;
  error?: boolean;
  errorMessage?: string;
  label?: string;
  required?: boolean;
  size?: "small" | "medium" | "large";
  showSeconds?: boolean;
  use12Hours?: boolean;
}

export function TimePicker({
  value,
  onChange,
  placeholder = "Select time",
  disabled = false,
  format = "12h",
  minTime,
  maxTime,
  step = 15,
  className = "",
  style = {},
  error = false,
  errorMessage,
  label,
  required = false,
  size = "medium",
  showSeconds = false,
  use12Hours = true,
}: TimePickerProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [inputValue, setInputValue] = useState("");
  const [selectedTime, setSelectedTime] = useState({
    hours: 0,
    minutes: 0,
    seconds: 0,
    ampm: "AM",
  });
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Format time for display
  const formatTime = (date: Date): string => {
    const hours = date.getHours();
    const minutes = date.getMinutes();
    const seconds = date.getSeconds();

    if (use12Hours) {
      const displayHours = hours === 0 ? 12 : hours > 12 ? hours - 12 : hours;
      const ampm = hours >= 12 ? "PM" : "AM";

      if (showSeconds) {
        return `${displayHours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}:${seconds.toString().padStart(2, "0")} ${ampm}`;
      }
      return `${displayHours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")} ${ampm}`;
    } else {
      if (showSeconds) {
        return `${hours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}:${seconds.toString().padStart(2, "0")}`;
      }
      return `${hours.toString().padStart(2, "0")}:${minutes.toString().padStart(2, "0")}`;
    }
  };

  // Parse time from input
  const parseTime = (timeString: string): Date | null => {
    if (!timeString) return null;

    // Try to parse various time formats
    const patterns = [
      /^(\d{1,2}):(\d{2})(?::(\d{2}))?\s*(AM|PM)?$/i,
      /^(\d{1,2}):(\d{2})(?::(\d{2}))?$/,
    ];

    for (const pattern of patterns) {
      const match = timeString.match(pattern);
      if (match) {
        let hours = parseInt(match[1], 10);
        const minutes = parseInt(match[2], 10);
        const seconds = match[3] ? parseInt(match[3], 10) : 0;
        const ampm = match[4]?.toUpperCase();

        if (use12Hours && ampm) {
          if (ampm === "PM" && hours !== 12) {
            hours += 12;
          } else if (ampm === "AM" && hours === 12) {
            hours = 0;
          }
        }

        const date = new Date();
        date.setHours(hours, minutes, seconds, 0);
        return date;
      }
    }

    return null;
  };

  // Initialize input value
  useEffect(() => {
    if (value) {
      setInputValue(formatTime(value));
      const hours = value.getHours();
      const minutes = value.getMinutes();
      const seconds = value.getSeconds();
      setSelectedTime({
        hours: use12Hours ? hours % 12 || 12 : hours,
        minutes,
        seconds,
        ampm: hours >= 12 ? "PM" : "AM",
      });
    } else {
      setInputValue("");
    }
  }, [value, use12Hours, showSeconds]);

  // Close picker when clicking outside
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  // Handle input change
  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    setInputValue(newValue);

    const parsedTime = parseTime(newValue);
    if (parsedTime) {
      onChange?.(parsedTime);
    }
  };

  // Handle time selection
  const handleTimeSelect = (
    hours: number,
    minutes: number,
    seconds: number = 0,
  ) => {
    let finalHours = hours;

    if (use12Hours) {
      if (selectedTime.ampm === "PM" && hours !== 12) {
        finalHours = hours + 12;
      } else if (selectedTime.ampm === "AM" && hours === 12) {
        finalHours = 0;
      }
    }

    const finalDate = new Date();
    finalDate.setHours(finalHours, minutes, seconds, 0);
    onChange?.(finalDate);
    setInputValue(formatTime(finalDate));
    setIsOpen(false);
  };

  // Handle time change
  const handleTimeChange = (
    field: "hours" | "minutes" | "seconds" | "ampm",
    value: string | number,
  ) => {
    setSelectedTime((prev) => ({
      ...prev,
      [field]: value,
    }));
  };

  // Generate time options
  const generateTimeOptions = () => {
    const options = [];
    const maxHours = use12Hours ? 12 : 23;
    const minHours = use12Hours ? 1 : 0;

    for (let hour = minHours; hour <= maxHours; hour++) {
      for (let minute = 0; minute < 60; minute += step) {
        const timeString = `${hour.toString().padStart(2, "0")}:${minute.toString().padStart(2, "0")}`;
        const timeValue = `${hour}:${minute}`;

        // Check if time is within min/max range
        if (minTime || maxTime) {
          const currentTime = `${hour.toString().padStart(2, "0")}:${minute.toString().padStart(2, "0")}`;
          if (minTime && currentTime < minTime) continue;
          if (maxTime && currentTime > maxTime) continue;
        }

        options.push({
          value: timeValue,
          label: timeString,
          hours: hour,
          minutes: minute,
        });
      }
    }

    return options;
  };

  // Navigate time
  const navigateTime = (
    direction: "up" | "down",
    field: "hours" | "minutes" | "seconds",
  ) => {
    setSelectedTime((prev) => {
      const newTime = { ...prev };
      const stepValue = field === "hours" ? 1 : field === "minutes" ? step : 1;
      const maxValue =
        field === "hours"
          ? use12Hours
            ? 12
            : 23
          : field === "minutes"
            ? 59
            : 59;
      const minValue = field === "hours" ? (use12Hours ? 1 : 0) : 0;

      if (direction === "up") {
        newTime[field] = Math.min(newTime[field] + stepValue, maxValue);
      } else {
        newTime[field] = Math.max(newTime[field] - stepValue, minValue);
      }

      return newTime;
    });
  };

  const sizeStyles = {
    small: { height: "32px", fontSize: "12px" },
    medium: { height: "40px", fontSize: "14px" },
    large: { height: "48px", fontSize: "16px" },
  };

  const currentSizeStyle = sizeStyles[size];

  return (
    <div
      ref={containerRef}
      className={className}
      style={{ position: "relative", ...style }}
    >
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

      <div style={{ position: "relative" }}>
        <div onClick={() => !disabled && setIsOpen(!isOpen)}>
          <Input
            value={inputValue}
            onChange={handleInputChange}
            placeholder={placeholder}
            disabled={disabled}
            style={{
              ...currentSizeStyle,
              paddingRight: "40px",
              cursor: "pointer",
            }}
          />
        </div>

        <button
          type="button"
          style={{
            position: "absolute",
            right: "8px",
            top: "50%",
            transform: "translateY(-50%)",
            background: "none",
            border: "none",
            color: tokens.color.neutral[400].value,
            cursor: disabled ? "not-allowed" : "pointer",
            padding: "4px",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
          onClick={() => !disabled && setIsOpen(!isOpen)}
          disabled={disabled}
        >
          <Icon name="clock" size="sm" />
        </button>
      </div>

      {error && errorMessage && (
        <div
          style={{
            fontSize: "12px",
            color: tokens.color.semantic.error.value,
            marginTop: "4px",
          }}
        >
          {errorMessage}
        </div>
      )}

      {isOpen && (
        <div
          style={{
            position: "absolute",
            top: "100%",
            left: 0,
            right: 0,
            zIndex: 1000,
            background: "var(--color-background-primary)",
            border: `1px solid ${tokens.color.neutral[600].value}`,
            borderRadius: "8px",
            boxShadow: "0 4px 12px rgba(0, 0, 0, 0.3)",
            marginTop: "4px",
            padding: "16px",
            minWidth: "200px",
          }}
        >
          {/* Time Display */}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              gap: "8px",
              marginBottom: "16px",
              padding: "12px",
              background: "var(--color-background-secondary)",
              borderRadius: "6px",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: "4px" }}>
              <button
                type="button"
                onClick={() => navigateTime("up", "hours")}
                style={{
                  background: "none",
                  border: "none",
                  color: tokens.color.neutral[400].value,
                  cursor: "pointer",
                  padding: "4px",
                }}
              >
                <Icon name="chevron-up" size="sm" />
              </button>
              <span
                style={{
                  fontSize: "18px",
                  fontWeight: "600",
                  color: "#FFFFFF",
                  minWidth: "40px",
                  textAlign: "center",
                }}
              >
                {selectedTime.hours.toString().padStart(2, "0")}
              </span>
              <button
                type="button"
                onClick={() => navigateTime("down", "hours")}
                style={{
                  background: "none",
                  border: "none",
                  color: tokens.color.neutral[400].value,
                  cursor: "pointer",
                  padding: "4px",
                }}
              >
                <Icon name="chevron-down" size="sm" />
              </button>
            </div>

            <span
              style={{
                color: tokens.color.neutral[400].value,
                fontSize: "18px",
              }}
            >
              :
            </span>

            <div style={{ display: "flex", alignItems: "center", gap: "4px" }}>
              <button
                type="button"
                onClick={() => navigateTime("up", "minutes")}
                style={{
                  background: "none",
                  border: "none",
                  color: tokens.color.neutral[400].value,
                  cursor: "pointer",
                  padding: "4px",
                }}
              >
                <Icon name="chevron-up" size="sm" />
              </button>
              <span
                style={{
                  fontSize: "18px",
                  fontWeight: "600",
                  color: "#FFFFFF",
                  minWidth: "40px",
                  textAlign: "center",
                }}
              >
                {selectedTime.minutes.toString().padStart(2, "0")}
              </span>
              <button
                type="button"
                onClick={() => navigateTime("down", "minutes")}
                style={{
                  background: "none",
                  border: "none",
                  color: tokens.color.neutral[400].value,
                  cursor: "pointer",
                  padding: "4px",
                }}
              >
                <Icon name="chevron-down" size="sm" />
              </button>
            </div>

            {showSeconds && (
              <>
                <span
                  style={{
                    color: tokens.color.neutral[400].value,
                    fontSize: "18px",
                  }}
                >
                  :
                </span>
                <div
                  style={{ display: "flex", alignItems: "center", gap: "4px" }}
                >
                  <button
                    type="button"
                    onClick={() => navigateTime("up", "seconds")}
                    style={{
                      background: "none",
                      border: "none",
                      color: tokens.color.neutral[400].value,
                      cursor: "pointer",
                      padding: "4px",
                    }}
                  >
                    <Icon name="chevron-up" size="sm" />
                  </button>
                  <span
                    style={{
                      fontSize: "18px",
                      fontWeight: "600",
                      color: "#FFFFFF",
                      minWidth: "40px",
                      textAlign: "center",
                    }}
                  >
                    {selectedTime.seconds.toString().padStart(2, "0")}
                  </span>
                  <button
                    type="button"
                    onClick={() => navigateTime("down", "seconds")}
                    style={{
                      background: "none",
                      border: "none",
                      color: tokens.color.neutral[400].value,
                      cursor: "pointer",
                      padding: "4px",
                    }}
                  >
                    <Icon name="chevron-down" size="sm" />
                  </button>
                </div>
              </>
            )}

            {use12Hours && (
              <div
                style={{ display: "flex", flexDirection: "column", gap: "2px" }}
              >
                <button
                  type="button"
                  onClick={() => handleTimeChange("ampm", "AM")}
                  style={{
                    background:
                      selectedTime.ampm === "AM"
                        ? tokens.color.brand[600].value
                        : "transparent",
                    border: "none",
                    color:
                      selectedTime.ampm === "AM"
                        ? "#000000"
                        : tokens.color.neutral[400].value,
                    cursor: "pointer",
                    padding: "4px 8px",
                    borderRadius: "4px",
                    fontSize: "12px",
                    fontWeight: "600",
                  }}
                >
                  AM
                </button>
                <button
                  type="button"
                  onClick={() => handleTimeChange("ampm", "PM")}
                  style={{
                    background:
                      selectedTime.ampm === "PM"
                        ? tokens.color.brand[600].value
                        : "transparent",
                    border: "none",
                    color:
                      selectedTime.ampm === "PM"
                        ? "#000000"
                        : tokens.color.neutral[400].value,
                    cursor: "pointer",
                    padding: "4px 8px",
                    borderRadius: "4px",
                    fontSize: "12px",
                    fontWeight: "600",
                  }}
                >
                  PM
                </button>
              </div>
            )}
          </div>

          {/* Quick Time Options */}
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(3, 1fr)",
              gap: "4px",
              marginBottom: "16px",
            }}
          >
            {["00:00", "06:00", "12:00", "18:00"].map((time) => {
              const [hours, minutes] = time.split(":").map(Number);
              return (
                <button
                  key={time}
                  type="button"
                  onClick={() => handleTimeSelect(hours, minutes)}
                  style={{
                    background: "var(--color-background-secondary)",
                    border: "none",
                    color: "#FFFFFF",
                    padding: "8px",
                    borderRadius: "4px",
                    cursor: "pointer",
                    fontSize: "12px",
                  }}
                >
                  {time}
                </button>
              );
            })}
          </div>

          {/* Action buttons */}
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              gap: "8px",
            }}
          >
            <Button
              variant="secondary"
              onClick={() => {
                onChange?.(null);
                setInputValue("");
                setIsOpen(false);
              }}
            >
              Clear
            </Button>
            <Button
              variant="primary"
              onClick={() =>
                handleTimeSelect(
                  selectedTime.hours,
                  selectedTime.minutes,
                  selectedTime.seconds,
                )
              }
            >
              Done
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
