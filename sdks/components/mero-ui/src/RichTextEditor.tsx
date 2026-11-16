import React, {
  forwardRef,
  useRef,
  useEffect,
  useState,
  useCallback,
} from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { useEditor, EditorContent } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { TextStyle } from "@tiptap/extension-text-style";
import { Color } from "@tiptap/extension-color";
import { TextAlign } from "@tiptap/extension-text-align";
import { Placeholder } from "@tiptap/extensions";
// Using default Tiptap icons instead of custom ones

export interface RichTextEditorProps {
  value?: string;
  defaultValue?: string;
  placeholder?: string;
  disabled?: boolean;
  readOnly?: boolean;
  required?: boolean;
  error?: boolean;
  helperText?: string;
  label?: string;
  className?: string;
  style?: React.CSSProperties;
  size?: "sm" | "md" | "lg";
  variant?: "default" | "filled" | "outlined";
  minHeight?: number;
  maxHeight?: number;
  onChange?: (html: string) => void;
  onSend?: (html: string) => void;
  sendOnEnter?: boolean;
  clearOnSend?: boolean;
  onFocus?: (event: React.FocusEvent<HTMLDivElement>) => void;
  onBlur?: (event: React.FocusEvent<HTMLDivElement>) => void;
  onSelectionChange?: (selection: Selection | null) => void;
  toolbar?: boolean;
  customToolbar?: React.ReactNode;
}

export interface RichTextEditorRef {
  insertContent: (content: string) => void;
  focus: () => void;
  getHTML: () => string;
}

export const RichTextEditor = forwardRef<RichTextEditorRef, RichTextEditorProps>(
  (
    {
      value,
      defaultValue,
      placeholder,
      disabled = false,
      readOnly = false,
      required = false,
      error = false,
      helperText,
      label,
      className = "",
      style = {},
      size = "md",
      variant = "default",
      minHeight = 120,
      maxHeight = 400,
      onChange,
      onSend,
      sendOnEnter = false,
      clearOnSend = true,
      onFocus,
      onBlur,
      onSelectionChange,
      toolbar = true,
      customToolbar,
    },
    ref,
  ) => {
    const editorRef = useRef<HTMLDivElement>(null);
    const [isFocused, setIsFocused] = useState(false);
    const [currentValue, setCurrentValue] = useState(
      value || defaultValue || "",
    );
    const lastSelectionRef = useRef<{ from: number; to: number } | null>(null);
    // Read by the Placeholder extension through a callback so a changed
    // `placeholder` prop takes effect without tearing down the editor (which
    // would drop selection and undo history). `useEditor` captures its options
    // once, so the value has to be reached indirectly.
    const placeholderRef = useRef(placeholder);
    placeholderRef.current = placeholder;

    // Function to clean up empty content
    const cleanEmptyContent = (html: string): string => {
      if (!html) return '';
      
      // Remove all empty paragraphs and normalize whitespace
      let cleaned = html
        .replace(/<p><\/p>/g, '')
        .replace(/<p><br><\/p>/g, '')
        .replace(/<p><br\/><\/p>/g, '')
        .replace(/<p>\s*<\/p>/g, '')
        .replace(/<p>\s*<br\s*\/?>\s*<\/p>/g, '')
        .trim();
      
      // If only whitespace or empty tags remain, return empty string
      if (!cleaned || cleaned === '<p></p>' || cleaned === '<p><br></p>' || cleaned === '<p><br/></p>') {
        return '';
      }
      
      return cleaned;
    };

    // Function to clean editor content directly
    const cleanEditorContent = () => {
      if (!editor) return;
      
      const html = editor.getHTML();
      const cleaned = cleanEmptyContent(html);
      
      if (cleaned !== html) {
        editor.commands.setContent(cleaned);
      }
    };

    const editor = useEditor({
      extensions: [
        // `link` and `underline` are configured THROUGH StarterKit, not
        // registered beside it.
        //
        // StarterKit already bundles both. Adding `Underline` and `Link` to
        // this array as well registered a second, different instance of each,
        // and ProseMirror rejects that outright:
        //
        //   [tiptap warn]: Duplicate extension names found: ['link','underline']
        //   RangeError: Adding different instances of a keyed plugin (plugin$)
        //
        // The RangeError is thrown while the editor is being constructed, so
        // the whole component fails to mount — it surfaces to the app as a
        // bare React error #520 with no mention of tiptap, which is why this
        // sat unexplained.
        StarterKit.configure({
          link: {
            openOnClick: false,
            HTMLAttributes: {
              class: "rich-text-link",
            },
          },
        }),
        // The `p.is-editor-empty:first-child::before` rule below has always
        // been here, but nothing ever emitted `data-placeholder` for it to
        // read — the extension was never registered, so `placeholder` was
        // accepted and silently ignored.
        Placeholder.configure({
          placeholder: () => placeholderRef.current ?? "",
        }),
        TextStyle,
        Color,
        TextAlign.configure({
          types: ["heading", "paragraph"],
        }),
      ],
      content: currentValue,
      editable: !disabled && !readOnly,
      onUpdate: ({ editor }) => {
        const html = editor.getHTML();
        const cleanedHtml = cleanEmptyContent(html);
        
        // Only update state if content actually changed
        if (cleanedHtml !== currentValue) {
          setCurrentValue(cleanedHtml);
          onChange?.(cleanedHtml);
        }
        
        // Only clean editor if it's truly empty (no content at all)
        if (cleanedHtml === '' && html !== '') {
          // Use setTimeout to avoid interfering with current operation
          setTimeout(() => {
            if (editor && editor.getHTML() !== '') {
              editor.commands.setContent('');
            }
          }, 0);
        }
      },
      onSelectionUpdate: ({ editor }) => {
        const selection = window.getSelection();
        // Track last selection positions to restore before toolbar actions
        const state: any = editor.state;
        if (state?.selection) {
          lastSelectionRef.current = {
            from: state.selection.from,
            to: state.selection.to,
          };
        }
        onSelectionChange?.(selection);
      },
      editorProps: {
        attributes: {
          class: "rich-text-editor-content",
          style: "outline: none; min-height: inherit;",
        },
        handleKeyDown: (_view, event) => {

          if (!sendOnEnter || disabled || readOnly) {
            return false;
          }
          // Shift+Enter: when inside a list, create a new list item
          if (event.key === "Enter" && event.shiftKey) {
            const state: any = editor?.state;
            const $from = state?.selection?.$from;
            const isEmptyParent =
              $from?.parent && $from.parent.textContent.length === 0;
            if (editor?.isActive("listItem")) {
              event.preventDefault();
              if (isEmptyParent) {
                // Exit list on empty item (like double Enter in lists)
                const lifted = editor.chain().liftListItem("listItem").run();
                if (!lifted) {
                  editor.chain().toggleBulletList().run();
                }
                return true;
              }
              const handled = editor.chain().splitListItem("listItem").run();
              if (handled) return true;
              return false;
            }
            if (editor?.isActive("blockquote")) {
              const nodeBefore = $from?.nodeBefore;
              const isAfterHardBreak =
                nodeBefore &&
                nodeBefore.type &&
                nodeBefore.type.name === "hardBreak";
              if (isEmptyParent || isAfterHardBreak) {
                event.preventDefault();
                // Split current (empty) line, then unwrap the new line to paragraph
                editor
                  .chain()
                  .splitBlock()
                  .toggleBlockquote()
                  .setParagraph()
                  .run();
                return true;
              }
              return false;
            }
            return false;
          }
          if (event.key === "Enter") {
            // Only send when cursor is in a TOP-LEVEL paragraph (depth === 1)
            const state: any = editor?.state;
            if (!state) return false;
            const $from = state.selection.$from;
            const inTopLevelParagraph =
              $from.depth === 1 &&
              $from.parent &&
              $from.parent.type &&
              $from.parent.type.name === "paragraph";
            if (!inTopLevelParagraph) return false;
            event.preventDefault();
            const html = editor?.getHTML() ?? "";
            onSend?.(html);
            if (clearOnSend) {
              editor?.commands.clearContent();
            }
            return true;
          }
          return false;
        },
      },
    });

    // Update editor content when external value changes
    useEffect(() => {
      if (value !== undefined && editor) {
        const currentHtml = cleanEmptyContent(editor.getHTML());
        const cleanedValue = cleanEmptyContent(value);
        if (currentHtml !== cleanedValue) {
          editor.commands.setContent(cleanedValue);
        }
      }
    }, [value, editor]);

    // Update internal value when external value changes
    useEffect(() => {
      if (value !== undefined) {
        setCurrentValue(cleanEmptyContent(value));
      }
    }, [value]);

    // Placeholder decorations are recomputed on transactions, not on React
    // renders. Changing the prop alone would leave the previous text on screen
    // until the user next typed, so nudge the editor with an empty transaction.
    useEffect(() => {
      if (!editor || editor.isDestroyed) return;
      editor.view.dispatch(editor.state.tr);
    }, [placeholder, editor]);


    const sizeStyles = {
      sm: {
        padding: "6px 8px",
        fontSize: "14px",
        minHeight: `${Math.max(minHeight, 80)}px`,
      },
      md: {
        padding: "8px 12px",
        fontSize: "16px",
        minHeight: `${Math.max(minHeight, 120)}px`,
      },
      lg: {
        padding: "12px 16px",
        fontSize: "18px",
        minHeight: `${Math.max(minHeight, 160)}px`,
      },
    };

    const variantStyles = {
      default: {
        backgroundColor: "#111111",
        borderWidth: "1px",
        borderStyle: "solid",
        borderColor: error ? tokens.color.semantic.error.value : "#2A2A2A",
      },
      filled: {
        backgroundColor: "#121212",
        borderWidth: "1px",
        borderStyle: "solid",
        borderColor: error ? tokens.color.semantic.error.value : "#1F1F1F",
      },
      outlined: {
        backgroundColor: "#0F0F0F",
        borderWidth: "1px",
        borderStyle: "solid",
        borderColor: error ? tokens.color.semantic.error.value : "#303033",
      },
    };

    const baseStyle: React.CSSProperties = {
      width: "100%",
      fontFamily: "var(--font-body)",
      color: "#FFFFFF",
      borderRadius: tokens.radius.md.value,
      outline: "none",
      transition: "all 0.2s ease",
      fontWeight: "400",
      lineHeight: "1.5",
      overflow: "auto",
      ...sizeStyles[size],
      ...variantStyles[variant],
      ...style,
    };

    const focusStyle = {
      borderColor: error
        ? tokens.color.semantic.error.value
        : tokens.color.brand[600].value,
      boxShadow: error
        ? "0 0 0 2px rgba(255,0,0,0.2)"
        : "0 0 0 2px rgba(165,255,17,0.18)",
    };

    const disabledStyle = {
      backgroundColor: "transparent",
      color: tokens.color.neutral[300].value,
      cursor: "not-allowed",
      opacity: 0.6,
    };

    const labelStyle: React.CSSProperties = {
      display: "block",
      fontSize: "14px",
      fontWeight: 500,
      color: "#FFFFFF",
      marginBottom: "4px",
      fontFamily: "var(--font-body)",
      lineHeight: "1.4",
    };

    const helperTextStyle: React.CSSProperties = {
      fontSize: "12px",
      color: error
        ? tokens.color.semantic.error.value
        : tokens.color.neutral[300].value,
      marginTop: "4px",
      fontFamily: "var(--font-body)",
      lineHeight: "1.4",
    };

    const containerStyle: React.CSSProperties = {
      display: "flex",
      flexDirection: "column",
      width: "100%",
    };

    const toolbarStyle: React.CSSProperties = {
      display: "flex",
      alignItems: "center",
      gap: "6px",
      padding: "6px 8px",
      backgroundColor: "transparent",
      borderWidth: "0 0 1px 0",
      borderStyle: "none none solid none",
      borderColor: "transparent transparent #222 transparent",
      borderRadius: `${tokens.radius.md.value} ${tokens.radius.md.value} 0 0`,
      flexWrap: "wrap",
    };

    const toolbarButtonStyle: React.CSSProperties = {
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      padding: "6px 8px",
      backgroundColor: "transparent",
      borderWidth: "1px",
      borderStyle: "solid",
      borderColor: "transparent",
      borderRadius: "6px",
      color: "#EDEDED",
      cursor: "pointer",
      fontSize: "13px",
      fontFamily: "var(--font-body)",
      transition: "background-color 0.15s ease, border-color 0.15s ease",
      minWidth: "28px",
      height: "28px",
    };

    const separatorStyle: React.CSSProperties = {
      width: "1px",
      height: "20px",
      backgroundColor: tokens.color.neutral[600].value,
      margin: "0 4px",
    };

    const editorStyle: React.CSSProperties = {
      ...baseStyle,
      maxHeight: `${maxHeight}px`,
      ...(disabled ? disabledStyle : {}),
      ...(isFocused ? focusStyle : {}),
    };

    const handleFocus = useCallback(
      (e: React.FocusEvent<HTMLDivElement>) => {
        setIsFocused(true);
        onFocus?.(e);
      },
      [onFocus],
    );

    const handleBlur = useCallback(
      (e: React.FocusEvent<HTMLDivElement>) => {
        setIsFocused(false);
        // Only clean up if editor is truly empty
        if (editor) {
          const html = editor.getHTML();
          const cleaned = cleanEmptyContent(html);
          if (cleaned === '' && html !== '') {
            editor.commands.setContent('');
          }
        }
        onBlur?.(e);
      },
      [onBlur, editor],
    );

    const executeCommand = useCallback(
      (command: () => void) => {
        if (editor && !disabled && !readOnly) {
          // Use ProseMirror selection directly; it persists across blur
          const sel: any = editor.state?.selection;
          const from = sel?.from;
          const to = sel?.to;
          editor.commands.focus();
          if (typeof from === "number" && typeof to === "number") {
            editor.commands.setTextSelection({ from, to });
          } else if (lastSelectionRef.current) {
            editor.commands.setTextSelection(lastSelectionRef.current);
          }
          command();
        }
      },
      [editor, disabled, readOnly],
    );

    const insertLink = useCallback(() => {
      const url = prompt("Enter URL:");
      if (url && editor) {
        editor.chain().focus().setLink({ href: url }).run();
      }
    }, [editor]);

    const changeTextColor = useCallback(() => {
      const color = prompt("Enter color (e.g., #ff0000 or red):");
      if (color && editor) {
        editor.chain().focus().setColor(color).run();
      }
    }, [editor]);

    const changeHighlightColor = useCallback(() => {
      const color = prompt("Enter highlight color (e.g., #ffff00 or yellow):");
      if (color && editor) {
        // For now, just set the text color as highlight functionality requires additional extension
        editor.chain().focus().setColor(color).run();
      }
    }, [editor]);

    const insertHeading = useCallback(() => {
      const level = prompt("Enter heading level (1-6):");
      if (level && ["1", "2", "3", "4", "5", "6"].includes(level) && editor) {
        editor
          .chain()
          .focus()
          .toggleHeading({ level: parseInt(level) as 1 | 2 | 3 | 4 | 5 | 6 })
          .run();
      }
    }, [editor]);

    const insertList = useCallback(() => {
      if (editor) {
        executeCommand(() => {
          const state: any = editor.state;
          const $from = state?.selection?.$from;
          const inParagraph = $from?.parent?.type?.name === "paragraph";
          const hasContentBeforeCursor =
            typeof $from?.parentOffset === "number" && $from.parentOffset > 0;
          const chain = editor.chain();
          if (inParagraph && hasContentBeforeCursor) {
            chain.splitBlock();
          }
          chain.toggleBulletList().run();
        });
      }
    }, [editor, executeCommand]);

    const insertOrderedList = useCallback(() => {
      if (editor) {
        executeCommand(() => {
          const state: any = editor.state;
          const $from = state?.selection?.$from;
          const inParagraph = $from?.parent?.type?.name === "paragraph";
          const hasContentBeforeCursor =
            typeof $from?.parentOffset === "number" && $from.parentOffset > 0;
          const chain = editor.chain();
          if (inParagraph && hasContentBeforeCursor) {
            chain.splitBlock();
          }
          chain.toggleOrderedList().run();
        });
      }
    }, [editor, executeCommand]);

    const insertQuote = useCallback(() => {
      if (editor) {
        executeCommand(() => {
          const state: any = editor.state;
          const $from = state?.selection?.$from;
          const inParagraph = $from?.parent?.type?.name === "paragraph";
          const hasContentBeforeCursor =
            typeof $from?.parentOffset === "number" && $from.parentOffset > 0;
          const chain = editor.chain();
          if (inParagraph && hasContentBeforeCursor) {
            chain.splitBlock();
          }
          chain.toggleBlockquote().run();
        });
      }
    }, [editor, executeCommand]);

    const insertCode = useCallback(() => {
      if (editor) {
        editor.chain().focus().toggleCodeBlock().run();
      }
    }, [editor]);

    // Expose methods via ref
    React.useImperativeHandle(ref, () => ({
      insertContent: (content: string) => {
        if (editor && !disabled && !readOnly) {
          editor.commands.focus();
          editor.commands.insertContent(content);
        }
      },
      focus: () => {
        if (editor) {
          editor.commands.focus();
        }
      },
      getHTML: () => {
        const html = editor?.getHTML() || "";
        return cleanEmptyContent(html);
      },
    }), [editor, disabled, readOnly]);

    const toolbarItems = [
      {
        label: "Bold",
        icon: "B",
        action: () =>
          executeCommand(() => editor?.chain().focus().toggleBold().run()),
        isActive: editor?.isActive("bold"),
      },
      {
        label: "Italic",
        icon: "I",
        action: () =>
          executeCommand(() => editor?.chain().focus().toggleItalic().run()),
        isActive: editor?.isActive("italic"),
      },
      {
        label: "Underline",
        icon: "U",
        action: () =>
          executeCommand(() => editor?.chain().focus().toggleUnderline().run()),
        isActive: editor?.isActive("underline"),
      },
      { type: "separator" },
      // {
      //   label: "Heading",
      //   icon: "H",
      //   action: insertHeading,
      //   isActive: editor?.isActive("heading"),
      // },
      {
        label: "List",
        icon: "•",
        action: insertList,
        isActive: editor?.isActive("bulletList"),
      },
      {
        label: "Ordered List",
        icon: "1.",
        action: insertOrderedList,
        isActive: editor?.isActive("orderedList"),
      },
      // {
      //   label: "Quote",
      //   icon: '"',
      //   action: insertQuote,
      //   isActive: editor?.isActive("blockquote"),
      // },
      {
        label: "Code",
        icon: "</>",
        action: insertCode,
        isActive: editor?.isActive("codeBlock"),
      },
      // { type: "separator" },
      // {
      //   label: "Link",
      //   icon: "🔗",
      //   action: insertLink,
      //   isActive: editor?.isActive("link"),
      // },
      // {
      //   label: "Color",
      //   icon: "A",
      //   action: changeTextColor,
      // },
      // {
      //   label: "Highlight",
      //   icon: "H",
      //   action: changeHighlightColor,
      // },
    ];

    const renderToolbarItem = useCallback(
      (item: any, index: number) => {
        if (item.type === "separator") {
          return <div key={`separator-${index}`} style={separatorStyle} />;
        }

        const isActive = item.isActive || false;

        return (
          <button
            key={`toolbar-${index}`}
            type="button"
            style={{
              ...toolbarButtonStyle,
              backgroundColor: isActive ? "rgba(165,255,17,0.15)" : "transparent",
              borderColor: isActive ? "rgba(165,255,17,0.35)" : "transparent",
            }}
            onMouseDown={(e) => {
              // Keep editor selection intact so actions apply at the cursor
              e.preventDefault();
              if (!(disabled || readOnly)) {
                item.action?.();
              }
            }}
            onMouseEnter={(e) => {
              if (!isActive) {
                e.currentTarget.style.backgroundColor = "#1a1a1a";
                e.currentTarget.style.borderColor = "#2a2a2a";
              }
            }}
            onMouseLeave={(e) => {
              if (!isActive) {
                e.currentTarget.style.backgroundColor = "transparent";
                e.currentTarget.style.borderColor = "transparent";
              }
            }}
            disabled={disabled || readOnly}
            title={item.label}
          >
            {item.icon}
          </button>
        );
      },
      [disabled, readOnly, toolbarButtonStyle],
    );

    if (!editor) {
      return null;
    }

    return (
      <div className={className} style={containerStyle}>
        {label && (
          <label style={labelStyle}>
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

        {toolbar && !customToolbar && (
          <div style={toolbarStyle}>{toolbarItems.map(renderToolbarItem)}</div>
        )}

        {customToolbar && <div style={toolbarStyle}>{customToolbar}</div>}

        <div
          ref={editorRef}
          style={editorStyle}
          onFocus={handleFocus}
          onBlur={handleBlur}
        >
          <EditorContent editor={editor} />
        </div>

        {helperText && <div style={helperTextStyle}>{helperText}</div>}

        <style>{`
        .rich-text-editor-content {
          background-color: transparent !important;
          color: #FFFFFF !important;
          border: none !important;
          outline: none !important;
          padding: 8px 12px !important;
          min-height: inherit !important;
        }
        
        .rich-text-editor-content:focus {
          outline: none !important;
        }
        
        /* Ensure ALL text elements are white by default */
        .rich-text-editor-content * {
          color: #FFFFFF !important;
        }
        
        /* Override for links only */
        .rich-text-editor-content a.rich-text-link {
          color: ${tokens.color.brand[600].value} !important;
          text-decoration: underline !important;
        }
        
        .rich-text-editor-content a.rich-text-link:hover {
          color: ${tokens.color.brand[100].value} !important;
        }
        
        .rich-text-editor-content h1, 
        .rich-text-editor-content h2, 
        .rich-text-editor-content h3, 
        .rich-text-editor-content h4, 
        .rich-text-editor-content h5, 
        .rich-text-editor-content h6 {
          color: #FFFFFF !important;
          margin: 0.5em 0 !important;
        }
        
        .rich-text-editor-content h1 { font-size: 1.5em !important; }
        .rich-text-editor-content h2 { font-size: 1.3em !important; }
        .rich-text-editor-content h3 { font-size: 1.1em !important; }
        .rich-text-editor-content h4 { font-size: 1em !important; }
        .rich-text-editor-content h5 { font-size: 0.9em !important; }
        .rich-text-editor-content h6 { font-size: 0.8em !important; }
        
        .rich-text-editor-content p {
          margin: 0.5em 0 !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content ul, 
        .rich-text-editor-content ol {
          margin: 0.5em 0 !important;
          padding-left: 1.5em !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content li {
          margin: 0.25em 0 !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content blockquote {
          margin: 0.5em 0 !important;
          padding: 0.5em 1em !important;
          border-left: 3px solid ${tokens.color.brand[600].value} !important;
          background-color: ${tokens.color.neutral[800].value} !important;
          font-style: italic !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content pre {
          margin: 0.5em 0 !important;
          padding: 0.5em !important;
          background-color: ${tokens.color.neutral[800].value} !important;
          border-radius: 4px !important;
          font-family: 'Courier New', monospace !important;
          overflow-x: auto !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content code {
          background-color: ${tokens.color.neutral[800].value} !important;
          padding: 0.2em 0.4em !important;
          border-radius: 3px !important;
          font-family: 'Courier New', monospace !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content strong, 
        .rich-text-editor-content b {
          font-weight: bold !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content em, 
        .rich-text-editor-content i {
          font-style: italic !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content u {
          text-decoration: underline !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content s, 
        .rich-text-editor-content strike {
          text-decoration: line-through !important;
          color: #FFFFFF !important;
        }
        
        .rich-text-editor-content p.is-editor-empty:first-child::before {
          content: attr(data-placeholder);
          float: left;
          color: ${tokens.color.neutral[400].value};
          pointer-events: none;
          height: 0;
        }
      `}</style>
      </div>
    );
  },
);

RichTextEditor.displayName = "RichTextEditor";