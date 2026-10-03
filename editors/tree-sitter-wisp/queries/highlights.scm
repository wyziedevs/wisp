; Patterns do not overlap, so editors that let the first match win and
; editors that let the last win agree.

(comment) @comment

(doctype) @keyword

((tag_name) @tag
  (#match? @tag "^[a-z]"))

((tag_name) @type
  (#match? @type "^[A-Z]"))

(attribute_name) @attribute

(directive_name) @keyword

(attribute_value) @string

(quoted_attribute_value
  [
    "\""
    "'"
  ] @string)

(directive
  [
    "\""
    "'"
  ] @string)

[
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket

"=" @operator

(frontmatter
  "---" @punctuation.delimiter)

(hole
  [
    "{"
    "}"
  ] @punctuation.special)

(client_hole
  [
    "{:"
    "}"
  ] @punctuation.special)

[
  (block_open)
  (branch_open)
  (block_end)
  (tag_open)
] @keyword

(block_start
  "}" @keyword)

(branch
  "}" @keyword)

(tag
  "}" @keyword)

(client_tag
  "}" @keyword)
