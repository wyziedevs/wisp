# prettier-plugin-wisp

Prettier formats Wisp `.wisp` files with it, for teams that format
everything with Prettier. The plugin has no code of its own for the
layout: it runs `wisp fmt --stdin`, so Prettier and `wisp fmt` always agree
(markup, the `---` block through rustfmt, scripts and styles).

Needs the `wisp` command. Without it, files are left as they are and
Prettier prints one warning.

```sh
npm install --save-dev prettier prettier-plugin-wisp
```

`.prettierrc`:

```json
{ "plugins": ["prettier-plugin-wisp"] }
```

```sh
npx prettier --write "src/**/*.wisp"
```

`wispPath` names another `wisp` (`{ "wispPath": "/opt/wisp/bin/wisp" }`).
Prettier's own options (`printWidth`, `tabWidth`…) do not apply: the layout
is Wisp's.

## License

MIT, Wyzie LLC. See [LICENSE](../../LICENSE).
