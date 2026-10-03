# Wisp in your editor

Two pieces, both in this folder:

- **`wisp lsp`**, the language server in the `wisp` CLI: problems as you
  type, hovers, go to definition, completion, formatting. Any editor with
  LSP runs it over stdio: command `wisp`, argument `lsp`.
- **[tree-sitter-wisp](tree-sitter-wisp)**, the grammar: highlighting with
  Rust, JavaScript, TypeScript and CSS embedded where they belong.

Plus [vscode](vscode) (an extension), [zed](zed) (an extension) and
[prettier-plugin-wisp](prettier-plugin-wisp) (Prettier formats `.wisp`
through `wisp fmt`). Formatting outside LSP: `wisp fmt --stdin [path]`
reads stdin and writes stdout.

The grammar lives at `https://github.com/wyziedevs/wisp`, folder
`editors/tree-sitter-wisp`; `REV` below is a commit of it:
`GRAMMAR_REV`.

## VS Code

See [vscode/README.md](vscode/README.md).

## Neovim

[nvim-lspconfig](https://github.com/neovim/nvim-lspconfig) and
[nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter)
(`master` branch):

```lua
vim.filetype.add({ extension = { wisp = 'wisp' } })

local configs = require('lspconfig.configs')
if not configs.wisp then
  configs.wisp = {
    default_config = {
      cmd = { 'wisp', 'lsp' },
      filetypes = { 'wisp' },
      root_dir = require('lspconfig.util').root_pattern('Cargo.toml'),
    },
  }
end
require('lspconfig').wisp.setup({})

require('nvim-treesitter.parsers').get_parser_configs().wisp = {
  install_info = {
    url = 'https://github.com/wyziedevs/wisp',
    location = 'editors/tree-sitter-wisp',
    files = { 'src/parser.c' },
    branch = 'main',
  },
  filetype = 'wisp',
}
```

Then `:TSInstall wisp` (and `rust javascript typescript css` for what is
embedded), and copy the queries where Neovim finds them:

```sh
mkdir -p ~/.config/nvim/queries/wisp
cp editors/tree-sitter-wisp/queries/*.scm ~/.config/nvim/queries/wisp/
```

Neovim 0.11 without nvim-lspconfig:
`vim.lsp.config('wisp', { cmd = { 'wisp', 'lsp' }, filetypes = { 'wisp' }, root_markers = { 'Cargo.toml' } })`
then `vim.lsp.enable('wisp')`. Format on save:
`vim.api.nvim_create_autocmd('BufWritePre', { pattern = '*.wisp', callback = function() vim.lsp.buf.format() end })`.

## Zed

The extension in [zed](zed) registers the language, the grammar and
`wisp lsp` (the `wisp` on `PATH`). Install it with **zed: install dev
extension** and pick `editors/zed`. Format on save is Zed's default.

## Helix

`~/.config/helix/languages.toml`:

```toml
[[language]]
name = "wisp"
scope = "text.html.wisp"
file-types = ["wisp"]
roots = ["Cargo.toml"]
language-servers = ["wisp"]
block-comment-tokens = { start = "<!--", end = "-->" }
indent = { tab-width = 2, unit = "  " }
auto-format = true

[language-server.wisp]
command = "wisp"
args = ["lsp"]

[[grammar]]
name = "wisp"
source = { git = "https://github.com/wyziedevs/wisp", rev = "REV", subpath = "editors/tree-sitter-wisp" }
```

```sh
hx --grammar fetch && hx --grammar build
mkdir -p ~/.config/helix/runtime/queries/wisp
cp editors/tree-sitter-wisp/queries/*.scm ~/.config/helix/runtime/queries/wisp/
```

## JetBrains (IntelliJ, RustRover, WebStorm…)

With the [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) plugin
(any edition): **Settings → Languages & Frameworks → Language Servers → +**,
name `Wisp`, command `wisp lsp`; under **Mappings → File name patterns**
add `*.wisp` with language id `wisp`.

For highlighting, **Settings → Editor → TextMate Bundles → +** and pick
`editors/vscode` (its TextMate grammar).

With the native LSP API (paid IDEs, 2023.2+), a plugin of one class:

```kotlin
class WispLsp : LspServerSupportProvider {
  override fun fileOpened(project: Project, file: VirtualFile, serverStarter: LspServerSupportProvider.LspServerStarter) {
    if (file.extension == "wisp") serverStarter.ensureServerStarted(WispServer(project))
  }
}
private class WispServer(project: Project) : ProjectWideLspServerDescriptor(project, "Wisp") {
  override fun isSupportedFile(file: VirtualFile) = file.extension == "wisp"
  override fun createCommandLine() = GeneralCommandLine("wisp", "lsp")
}
```

registered in `plugin.xml` as
`<platform.lsp.serverSupportProvider implementation="WispLsp"/>`.

## Sublime Text

Install [LSP](https://packagecontrol.io/packages/LSP). A syntax for
`.wisp`, `Packages/User/Wisp.sublime-syntax` (HTML underneath):

```yaml
%YAML 1.2
---
name: Wisp
scope: text.html.wisp
version: 2
extends: Packages/HTML/HTML.sublime-syntax
file_extensions: [wisp]
```

**Preferences → Package Settings → LSP → Settings**:

```json
{
  "clients": {
    "wisp": {
      "enabled": true,
      "command": ["wisp", "lsp"],
      "selector": "text.html.wisp"
    }
  },
  "lsp_format_on_save": true
}
```

## Emacs

Eglot (built in since Emacs 29). The mode is named `wisp-html-mode`, as
`wisp-mode` is a Scheme dialect's:

```elisp
(define-derived-mode wisp-html-mode mhtml-mode "Wisp")
(add-to-list 'auto-mode-alist '("\\.wisp\\'" . wisp-html-mode))
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs '(wisp-html-mode "wisp" "lsp")))
(add-hook 'wisp-html-mode-hook
          (lambda ()
            (eglot-ensure)
            (add-hook 'before-save-hook #'eglot-format-buffer nil t)))
```

## Prettier

See [prettier-plugin-wisp/README.md](prettier-plugin-wisp/README.md).
