; Rust on the server: the `---` block, `{expr}`, `{@tag …}`, `{#block …}`.
((frontmatter
  (code) @injection.content)
  (#set! injection.language "rust"))

((hole
  (code) @injection.content)
  (#set! injection.language "rust"))

((tag
  (code) @injection.content)
  (#set! injection.language "rust"))

((block
  (block_start
    (code) @injection.content))
  (#set! injection.language "rust"))

((block
  (branch
    (code) @injection.content))
  (#set! injection.language "rust"))

; JavaScript in the browser: `{:expr}`, `{:@render …}`, `{:#block …}`, and
; directive values (`on:click="count++"`).
((client_hole
  (code) @injection.content)
  (#set! injection.language "javascript"))

((client_tag
  (code) @injection.content)
  (#set! injection.language "javascript"))

((client_block
  (block_start
    (code) @injection.content))
  (#set! injection.language "javascript"))

((client_block
  (branch
    (code) @injection.content))
  (#set! injection.language "javascript"))

((directive
  (code) @injection.content)
  (#set! injection.language "javascript"))

; <script> is JavaScript, or TypeScript with lang="ts".
((script_element
  (start_tag) @_tag
  (raw_text) @injection.content)
  (#not-match? @_tag "lang=[\"']?(ts|typescript)[\"' >]")
  (#set! injection.language "javascript"))

((script_element
  (start_tag) @_tag
  (raw_text) @injection.content)
  (#match? @_tag "lang=[\"']?(ts|typescript)[\"' >]")
  (#set! injection.language "typescript"))

((style_element
  (raw_text) @injection.content)
  (#set! injection.language "css"))
