/**
 * Wisp `.wisp` files: an optional `---` Rust block, then HTML with
 * `{rust}` holes, `{#…}` blocks, `{:js}` holes, `{:#…}` client blocks and
 * directives. Tags are flat (start and end tags are separate nodes), so
 * markup that does not balance still parses; template blocks nest. The
 * embedded code is left whole for injections (queries/injections.scm).
 */

// Raw text up to the end tag `</name`, any case: no external scanner.
function rawText(name) {
  let alts = ['[^<]', '<[^/<]'];
  for (let i = 0; i < name.length; i++) {
    const seen = [...name.slice(0, i)].map((c) => `[${c}${c.toUpperCase()}]`).join('');
    const c = name[i];
    alts.push(`<\\/${seen}[^${c}${c.toUpperCase()}<]`);
  }
  return new RegExp(`(${alts.join('|')})+`);
}

module.exports = grammar({
  name: 'wisp',

  extras: ($) => [/\s/],

  word: ($) => $.tag_name,

  rules: {
    document: ($) => seq(optional($.frontmatter), repeat($._node)),

    // `---` lines around Rust. A content line is anything but `---`.
    frontmatter: ($) =>
      seq(
        alias(token(prec(2, /---[ \t]*\r?\n/)), '---'),
        optional(alias(/(([^-\r\n][^\n]*)?\r?\n|-([^-\r\n][^\n]*)?\r?\n|--([^-\r\n][^\n]*)?\r?\n|---[ \t]*[^ \t\r\n][^\n]*\n)+/, $.code)),
        '---',
      ),

    _node: ($) =>
      choice(
        $.text,
        $.comment,
        $.doctype,
        $.start_tag,
        $.self_closing_tag,
        $.end_tag,
        $.script_element,
        $.style_element,
        $.hole,
        $.client_hole,
        $.tag,
        $.client_tag,
        $.block,
        $.client_block,
      ),

    text: (_) => /[^<{\s][^<{]*/,

    comment: (_) => token(seq('<!--', /[^-]*-+([^->][^-]*-+)*/, '>')),

    doctype: (_) => /<![dD][oO][cC][tT][yY][pP][eE][^>]*>/,

    // Tags.
    tag_name: (_) => /[a-zA-Z][a-zA-Z0-9:._-]*/,

    start_tag: ($) => seq('<', $.tag_name, repeat($._attribute), '>'),

    self_closing_tag: ($) => seq('<', $.tag_name, repeat($._attribute), '/>'),

    end_tag: ($) => seq('</', $.tag_name, '>'),

    script_element: ($) =>
      seq(
        alias($.script_start_tag, $.start_tag),
        optional(alias(rawText('script'), $.raw_text)),
        alias($.script_end_tag, $.end_tag),
      ),

    script_start_tag: ($) => seq('<', alias('script', $.tag_name), repeat($._attribute), '>'),

    script_end_tag: ($) => seq('</', alias('script', $.tag_name), '>'),

    style_element: ($) =>
      seq(
        alias($.style_start_tag, $.start_tag),
        optional(alias(rawText('style'), $.raw_text)),
        alias($.style_end_tag, $.end_tag),
      ),

    style_start_tag: ($) => seq('<', alias('style', $.tag_name), repeat($._attribute), '>'),

    style_end_tag: ($) => seq('</', alias('style', $.tag_name), '>'),

    // Attributes: `name="text {rust}"`, `name={rust}`, `{name}`, and
    // directives, whose quoted value is JavaScript.
    _attribute: ($) => choice($.attribute, $.directive, $.hole, $.client_hole),

    attribute: ($) =>
      seq(
        $.attribute_name,
        optional(
          seq(
            '=',
            choice(
              $.quoted_attribute_value,
              alias(/[^\s"'=<>`{}]+/, $.attribute_value),
              $.hole,
              $.client_hole,
            ),
          ),
        ),
      ),

    attribute_name: (_) => /[^\s"'<>\/={}:][^\s"'<>\/={}]*/,

    quoted_attribute_value: ($) =>
      choice(
        seq('"', repeat(choice(alias(/[^"{]+/, $.attribute_value), $.hole, $.client_hole)), '"'),
        seq("'", repeat(choice(alias(/[^'{]+/, $.attribute_value), $.hole, $.client_hole)), "'"),
      ),

    directive: ($) =>
      seq(
        $.directive_name,
        optional(
          seq(
            '=',
            choice(
              seq('"', optional(alias(/[^"]+/, $.code)), '"'),
              seq("'", optional(alias(/[^']+/, $.code)), "'"),
              $.hole,
            ),
          ),
        ),
      ),

    directive_name: (_) =>
      token(
        prec(
          1,
          choice(
            /(on|bind|class|style|use|transition|in|out|animate|client):[^\s"'<>\/=]+/,
            /:[a-zA-Z][^\s"'<>\/=]*/,
          ),
        ),
      ),

    // Holes: `{rust}`, `{:js}`, and the tags `{@html x}`, `{:@render s(x)}`.
    hole: ($) => seq('{', optional($.code), '}'),

    client_hole: ($) => seq('{:', optional($.code), '}'),

    tag: ($) => seq(alias(/\{@[a-zA-Z]+/, $.tag_open), optional($.code), '}'),

    client_tag: ($) => seq(alias(/\{:@[a-zA-Z]+/, $.tag_open), optional($.code), '}'),

    // Code: balanced braces, and double-quoted strings that may hold them.
    code: ($) => repeat1($._code_part),

    _code_part: ($) => choice(/[^{}"]+/, /"([^"\\]|\\.)*"/, seq('{', repeat($._code_part), '}')),

    // Blocks: `{#if c}…{:else}…{/if}` (Rust), `{:#each xs as x if c}…{:/each}` (JS;
    // `{/each}` ends it too).
    block: ($) =>
      seq(
        $.block_start,
        repeat($._node),
        repeat($.branch),
        $.block_end,
      ),

    client_block: ($) =>
      seq(
        alias($.client_block_start, $.block_start),
        repeat($._node),
        repeat($.branch),
        $.block_end,
      ),

    block_start: ($) => seq(alias(/\{#[a-zA-Z]+/, $.block_open), optional($.code), '}'),

    client_block_start: ($) => seq(alias(/\{:#[a-zA-Z]+/, $.block_open), optional($.code), '}'),

    branch: ($) =>
      prec.right(
        seq(
          alias(/\{:(else([ \t\r\n]+if)?|case|then|catch|finally)/, $.branch_open),
          optional($.code),
          '}',
          repeat($._node),
        ),
      ),

    block_end: (_) => /\{:?\/[a-zA-Z]+[ \t]*\}/,
  },
});
