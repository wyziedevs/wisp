# tree-sitter-wisp

A [tree-sitter](https://tree-sitter.github.io) grammar for Wisp `.wisp`
files, for Neovim, Helix, Zed and any editor that reads tree-sitter. No
external scanner: `src/parser.c` is all of it.

The markup is HTML with flat tags (start and end tags are separate nodes, so
markup that does not balance still parses) and nested template blocks. Code
is left whole for `queries/injections.scm`:

| Where | Language |
|---|---|
| the `---` block, `{expr}`, `{@html …}`, `{#if …}`, `{:else if …}` | Rust |
| `{:expr}`, `{:@render …}`, `{:#each …}`, directive values (`on:click="…"`) | JavaScript |
| `<script>` | JavaScript, TypeScript with `lang="ts"` |
| `<style>` | CSS |

Setup for each editor: [../README.md](../README.md).

## Work on it

```sh
cd editors/tree-sitter-wisp
npx tree-sitter-cli@0.25 generate --abi 14   # after editing grammar.js
npx tree-sitter-cli@0.25 test                # test/corpus/*.txt
npx tree-sitter-cli@0.25 parse some.wisp
```

Commit `src/` with `grammar.js`: editors build from `src/parser.c`. ABI 14
keeps older editors working.

## GitHub highlighting (Linguist)

GitHub names a file's language with [Linguist](https://github.com/github-linguist/linguist).
Until it knows Wisp, an app gets highlighting with a line in its
`.gitattributes`:

```
*.wisp linguist-language=Svelte
```

To add Wisp to Linguist (see its CONTRIBUTING.md; nothing is submitted yet):

1. `.wisp` is already Linguist's for [wisp](https://github.com/Gozala/wisp),
   a Lisp. The new language needs another name (`Wisp Template`) and a
   heuristic in `lib/linguist/heuristics.yml` that tells them apart: a
   template's first line is `---`, or it has `<` tags and `{#`/`{@` blocks;
   the Lisp starts with `(` or `;`.
2. Linguist only adds a language used in hundreds of public repositories
   (its "in the wild" rule, searched by extension). Wait for that.
3. Highlighting on github.com comes from a TextMate grammar: put
   `editors/vscode/syntaxes` in a repository of its own and add it with
   `script/add-grammar https://github.com/…/wisp-tmlanguage`. The entry in
   `lib/linguist/languages.yml` takes `extensions: [".wisp"]`,
   `tm_scope: text.html.wisp`, `ace_mode: html`, `type: markup`, a color and
   `language_id` from `script/update-ids`.
4. Add samples under `samples/Wisp Template/` (a page, a layout, a
   component) and run `script/bootstrap` and `bundle exec rake test`.
