# Wisp for VS Code

Highlighting for `.wisp` files (HTML, Rust in the `---` block and `{…}`,
JavaScript in `<script>`, directive values and `{:…}`, CSS in `<style>`),
and `wisp lsp`: problems as you type, hovers, go to definition and
completion.

## Install from source

Needs the `wisp` command on your `PATH` (or set `wisp.path`).

```sh
cd editors/vscode
npm install
npx @vscode/vsce package --skip-license
code --install-extension wisp-0.1.0.vsix
```
