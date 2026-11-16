---
"@calimero-network/mero-ui": patch
---

RichTextEditor: stop throwing on every mount

Every editor mount threw
`RangeError: Adding different instances of a keyed plugin (plugin$)`,
surfacing in consumers as a bare `Minified React error #520`. The build
inlined StarterKit, and with it a private copy of `@tiptap/core` and
ProseMirror, but left `@tiptap/extensions` (the `Placeholder` added in 1.5.1)
external. In a consumer that import resolved to a second copy from
`node_modules`, and two ProseMirror copies each name their first unkeyed
plugin `plugin$`. `@tiptap/extensions` is now bundled too, so the dist imports
no Tiptap package at all.

Also stops registering `link` and `underline` twice (#24): Tiptap v3's
`StarterKit` already bundles both, and adding them again logged
`[tiptap warn]: Duplicate extension names found: ['link','underline']`.
