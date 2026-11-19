import React, { useState, useCallback } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

interface TreeNode {
  id: string;
  label: string;
  children?: TreeNode[];
  icon?: React.ReactNode;
  disabled?: boolean;
  selected?: boolean;
  expanded?: boolean;
  data?: any;
}

interface TreeProps {
  data: TreeNode[];
  onNodeSelect?: (node: TreeNode) => void;
  onNodeToggle?: (node: TreeNode, expanded: boolean) => void;
  selectable?: boolean;
  expandable?: boolean;
  size?: "sm" | "md" | "lg";
  variant?: "default" | "minimal" | "detailed";
  className?: string;
  style?: React.CSSProperties;
}

export const Tree: React.FC<TreeProps> = ({
  data,
  onNodeSelect,
  onNodeToggle,
  selectable = true,
  expandable = true,
  size = "md",
  variant = "default",
  className = "",
  style = {},
}) => {
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(
    new Set(data.filter((node) => node.expanded).map((node) => node.id)),
  );
  const [selectedNode, setSelectedNode] = useState<string | null>(
    data.find((node) => node.selected)?.id || null,
  );

  const sizeStyles = {
    sm: {
      fontSize: "12px",
      iconSize: 16,
      itemHeight: "28px",
      indentSize: "16px",
    },
    md: {
      fontSize: "14px",
      iconSize: 18,
      itemHeight: "32px",
      indentSize: "20px",
    },
    lg: {
      fontSize: "16px",
      iconSize: 20,
      itemHeight: "36px",
      indentSize: "24px",
    },
  };

  const handleNodeToggle = useCallback(
    (node: TreeNode) => {
      if (!expandable || !node.children?.length) return;

      const newExpandedNodes = new Set(expandedNodes);
      if (expandedNodes.has(node.id)) {
        newExpandedNodes.delete(node.id);
      } else {
        newExpandedNodes.add(node.id);
      }

      setExpandedNodes(newExpandedNodes);
      onNodeToggle?.(node, newExpandedNodes.has(node.id));
    },
    [expandedNodes, expandable, onNodeToggle],
  );

  const handleNodeSelect = useCallback(
    (node: TreeNode) => {
      if (!selectable || node.disabled) return;

      setSelectedNode(node.id);
      onNodeSelect?.(node);
    },
    [selectable, onNodeSelect],
  );

  const renderTreeNode = (
    node: TreeNode,
    level: number = 0,
  ): React.ReactNode => {
    const isExpanded = expandedNodes.has(node.id);
    const isSelected = selectedNode === node.id;
    const hasChildren = node.children && node.children.length > 0;
    const canExpand = expandable && hasChildren;

    const nodeStyle: React.CSSProperties = {
      display: "flex",
      alignItems: "center",
      height: sizeStyles[size].itemHeight,
      paddingLeft: `${level * parseInt(sizeStyles[size].indentSize)}px`,
      cursor: node.disabled ? "not-allowed" : "pointer",
      backgroundColor: isSelected
        ? tokens.color.background.brand.value
        : "transparent",
      color: node.disabled
        ? tokens.color.neutral[300].value
        : isSelected
          ? tokens.color.neutral[200].value
          : tokens.color.background.primary.value,
      opacity: node.disabled ? 0.6 : 1,
      transition: "all 0.2s ease",
      borderRadius: tokens.radius.sm.value,
      margin: "1px 0",
    };

    const toggleStyle: React.CSSProperties = {
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      width: sizeStyles[size].iconSize,
      height: sizeStyles[size].iconSize,
      marginRight: tokens.space["2"].value,
      cursor: canExpand ? "pointer" : "default",
      borderRadius: tokens.radius.sm.value,
      transition: "background-color 0.2s ease",
    };

    const iconStyle: React.CSSProperties = {
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      width: sizeStyles[size].iconSize,
      height: sizeStyles[size].iconSize,
      marginRight: tokens.space["2"].value,
      color: "inherit",
    };

    const labelStyle: React.CSSProperties = {
      fontSize: sizeStyles[size].fontSize,
      fontWeight: isSelected ? 600 : 400,
      flex: 1,
      whiteSpace: "nowrap",
      overflow: "hidden",
      textOverflow: "ellipsis",
    };

    return (
      <div key={node.id}>
        <div
          style={nodeStyle}
          onClick={() => handleNodeSelect(node)}
          onMouseEnter={(e) => {
            if (!node.disabled && !isSelected) {
              e.currentTarget.style.backgroundColor =
                tokens.color.background.brand.value;
            }
          }}
          onMouseLeave={(e) => {
            if (!node.disabled && !isSelected) {
              e.currentTarget.style.backgroundColor = "transparent";
            }
          }}
        >
          {canExpand && (
            <div
              style={toggleStyle}
              onClick={(e) => {
                e.stopPropagation();
                handleNodeToggle(node);
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor =
                  tokens.color.background.tertiary.value;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = "transparent";
              }}
            >
              <Icon
                name={isExpanded ? "chevron-down" : "chevron-right"}
                size="sm"
                color="current"
                style={{
                  transform: isExpanded ? "rotate(0deg)" : "rotate(0deg)",
                  transition: "transform 0.2s ease",
                }}
              />
            </div>
          )}

          {!canExpand && (
            <div
              style={{
                width: sizeStyles[size].iconSize,
                marginRight: tokens.space["2"].value,
              }}
            />
          )}

          {node.icon && <div style={iconStyle}>{node.icon}</div>}

          <div style={labelStyle}>{node.label}</div>
        </div>

        {hasChildren && isExpanded && (
          <div>
            {node.children!.map((child) => renderTreeNode(child, level + 1))}
          </div>
        )}
      </div>
    );
  };

  const treeStyle: React.CSSProperties = {
    fontFamily: "system-ui, sans-serif",
    userSelect: "none",
    ...style,
  };

  return (
    <div className={className} style={treeStyle}>
      {data.map((node) => renderTreeNode(node))}
    </div>
  );
};
