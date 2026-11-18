import React from "react";

export type TabItem = { id: string; label: string; disabled?: boolean };

export type TabsProps = {
  tabs: TabItem[];
  value?: string;
  defaultValue?: string;
  onValueChange?: (id: string) => void;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
};

export function Tabs({
  tabs,
  value,
  defaultValue,
  onValueChange,
  className = "",
  style,
  children,
}: TabsProps) {
  const isControlled = typeof value === "string";
  const [internal, setInternal] = React.useState<string>(
    defaultValue || tabs[0]?.id,
  );
  const active = isControlled ? (value as string) : internal;

  const setActive = (id: string) => {
    if (!isControlled) setInternal(id);
    onValueChange?.(id);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const currentIndex = tabs.findIndex((t) => t.id === active);
    if (event.key === "ArrowRight") {
      const next = (currentIndex + 1) % tabs.length;
      setActive(tabs[next].id);
    } else if (event.key === "ArrowLeft") {
      const prev = (currentIndex - 1 + tabs.length) % tabs.length;
      setActive(tabs[prev].id);
    }
  };

  return (
    <div className={className} style={style}>
      <div
        role="tablist"
        aria-orientation="horizontal"
        onKeyDown={onKeyDown}
        style={{
          display: "flex",
          gap: "8px",
          borderBottom: "1px solid var(--color-neutral-600)",
          paddingBottom: "4px",
        }}
      >
        {tabs.map((tab) => {
          const isActive = tab.id === active;
          return (
            <button
              key={tab.id}
              role="tab"
              aria-selected={isActive}
              aria-controls={`panel-${tab.id}`}
              disabled={tab.disabled}
              onClick={() => setActive(tab.id)}
              style={{
                background: "transparent",
                border: "none",
                borderBottom: isActive
                  ? "2px solid var(--color-brand-600)"
                  : "2px solid transparent",
                color: isActive ? "#FFFFFF" : "var(--color-neutral-300)",
                padding: "8px 12px",
                fontFamily: "var(--font-body)",
                fontWeight: 600,
                fontSize: "13px",
                cursor: tab.disabled ? "not-allowed" : "pointer",
              }}
            >
              {tab.label}
            </button>
          );
        })}
      </div>
      {children}
    </div>
  );
}

export type TabPanelProps = {
  when: string;
  active?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
};

export function TabPanel({
  when,
  active,
  className = "",
  style,
  children,
}: TabPanelProps) {
  const isActive = active === when;
  return (
    <div
      role="tabpanel"
      id={`panel-${when}`}
      hidden={!isActive}
      className={className}
      style={{ paddingTop: "12px", ...style }}
    >
      {isActive ? children : null}
    </div>
  );
}
