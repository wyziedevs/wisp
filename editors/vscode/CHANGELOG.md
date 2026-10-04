# Changelog

## Unreleased

- Rust attributes in the `---` block (`#[action]`, `#[validate(min = 3)]`, `#![allow(x)]`) get their own scopes: name, arguments, strings, numbers.
- Hover and completion for Wisp's Rust attributes, `#[validate]` rules and `#[rest]` keys.

## 0.1.0

- Highlighting for `.wisp` files: HTML, Rust, JavaScript, TypeScript and CSS where they are embedded.
- `wisp lsp`: problems as you type, hovers, go to definition, completion, formatting (on save by default).
- Snippets and the purple ghost file icon.
