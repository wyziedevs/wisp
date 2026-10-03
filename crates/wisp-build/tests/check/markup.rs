//! What the template parser and the component checks refuse, reported as
//! `file:line:col` in the `.wisp` file the mistake is in.

use crate::common::{fails, page_fails};

#[test]
fn elements_and_head() {
    page_fails(&[
        (
            "head with attributes",
            "\n<wisp:head class=\"x\"></wisp:head>",
            &["src/routes/+page.wisp:2:1: <wisp:head> takes no attributes"],
        ),
        (
            "a head closed but never opened",
            "<p>x</p>\n</wisp:head>",
            &["src/routes/+page.wisp:2:1:", "</wisp:head>"],
        ),
        (
            "a head in a head",
            "<wisp:head><wisp:head></wisp:head></wisp:head>",
            &["src/routes/+page.wisp:1:12: <wisp:head> cannot be nested"],
        ),
        (
            "an unknown wisp element",
            "<wisp:nope />",
            &[
                "src/routes/+page.wisp:1:1: <wisp:nope> is not a Wisp element",
                "<wisp:element>",
            ],
        ),
        (
            "a window that is closed",
            "</wisp:window>",
            &["src/routes/+page.wisp:1:1: <wisp:window> closes itself: <wisp:window … />"],
        ),
        (
            "a window with a body",
            "<wisp:window on:click=\"x\">a</wisp:window>",
            &["<wisp:window> closes itself"],
        ),
        (
            "an element without its tag",
            "<wisp:element>x</wisp:element>",
            &[
                "src/routes/+page.wisp:1:1: <wisp:element> needs its tag",
                "this={:tag}",
            ],
        ),
        (
            "a closing tag with attributes",
            "<Card>x</Card y>",
            &["src/routes/+page.wisp:1:8: </Card> takes nothing but its name"],
        ),
        (
            "a closing tag that opens nothing",
            "x</Card>",
            &["src/routes/+page.wisp:1:2:", "</Card>"],
        ),
        (
            "an element left open",
            "<p>\n<Card>x",
            &["src/routes/+page.wisp:2:1:", "never closed"],
        ),
        (
            "a comment left open",
            "\n<!-- x",
            &["src/routes/+page.wisp:2:1: unclosed <!-- comment"],
        ),
        (
            "a tag left open",
            "<p>\n<a href=\"/x\"",
            &["src/routes/+page.wisp:2:1:", "unclosed <a"],
        ),
        (
            "an animation that is not flip",
            "{:#each xs as x (x)}<li animate:spin>{:x}</li>{/each}<script>let xs = []</script>",
            &[
                "src/routes/+page.wisp:1:",
                "`animate:spin` is not an animation: use animate:flip",
            ],
        ),
    ]);
}

#[test]
fn blocks_and_tags() {
    page_fails(&[
        (
            "text before the first case",
            "{#match x}<b>{:case _}y{/match}",
            &["src/routes/+page.wisp:1:1: only {:case …} may follow {#match …}"],
        ),
        (
            "a match with no case",
            "\n{#match x}{/match}",
            &["src/routes/+page.wisp:2:1: {#match} needs at least one {:case}"],
        ),
        (
            "an unknown block",
            "{#nope x}{/nope}",
            &["src/routes/+page.wisp:1:1: unknown block {#nope}"],
        ),
        (
            "a block without an expression",
            "{#if}x{/if}",
            &["src/routes/+page.wisp:1:1: {#if} needs an expression"],
        ),
        (
            "an each without as",
            "{#each xs}{/each}",
            &["src/routes/+page.wisp:1:1: expected {#each <expr> as <pattern>"],
        ),
        (
            "an else with something after it",
            "{#if a}x{:else foo}y{/if}",
            &["src/routes/+page.wisp:1:9: expected {:else} or {:else if <cond>}"],
        ),
        (
            "elseif",
            "{#if a}x{:elseif b}y{/if}",
            &["{:elseif} is written {:else if <condition>}"],
        ),
        (
            "a case outside a match",
            "{#if a}x{:case 1}y{/if}",
            &["{:case} is not allowed here"],
        ),
        (
            "an unknown at-tag",
            "{@nope x}",
            &["src/routes/+page.wisp:1:1: unknown or malformed {@nope …}"],
        ),
        (
            "a bare at-html",
            "{@html}",
            &["unknown or malformed {@html …}"],
        ),
        (
            "html in a tag",
            "<p {@html x}>",
            &["inside a tag, expressions must be attribute values"],
        ),
        (
            "html in an attribute",
            "<p title=\"{@html x}\">x</p>",
            &["src/routes/+page.wisp:1:", "@html"],
        ),
        (
            "an empty hole",
            "<p>{}</p>",
            &["src/routes/+page.wisp:1:4: empty {}"],
        ),
        (
            "a hole in a script-scheme url",
            "<a href=\"javascript:go({x})\">x</a>",
            &["no expressions in a `href` that runs script"],
        ),
        (
            "a hole after an encoded scheme",
            "<a href=\"java&#115;cript:{x}\">x</a>",
            &["`href` has a character reference"],
        ),
        (
            "srcdoc",
            "<iframe srcdoc=\"{x}\"></iframe>",
            &["no expressions in `srcdoc`"],
        ),
        (
            "a line comment in a hole",
            "<p>{x // why}</p>",
            &["no // comments inside {…}"],
        ),
        (
            "an event handler attribute",
            "<a onclick=\"go({x})\">x</a>",
            &["src/routes/+page.wisp:1:", "event handler"],
        ),
        ("a hole for a tag's name", "<p><{x}></p>", &["tag's name"]),
        (
            "an unclosed hole",
            "<p>{x</p>",
            &["src/routes/+page.wisp:1:4:", "unclosed {"],
        ),
    ]);
}

#[test]
fn browser_blocks_and_directives() {
    page_fails(&[
        (
            "an each with a bad item",
            "{:#each xs as 1}{/each}<script>let xs = []</script>",
            &["src/routes/+page.wisp:1:1: expected {:#each list as item}"],
        ),
        (
            "an each with a bad index",
            "{:#each xs as x, 1}{/each}<script>let xs = []</script>",
            &["expected {:#each list as item}"],
        ),
        (
            "a try with an argument",
            "{:#try x}a{:catch e}b{/try}",
            &["src/routes/+page.wisp:1:1: {:#try} takes nothing"],
        ),
        (
            "an await then a bad name",
            "{:#await p then 1x}a{/await}<script>let p</script>",
            &["`1x` is not a name for what the block gives"],
        ),
        (
            "an unknown browser block",
            "{:#nope x}a{/nope}",
            &["unknown block {:#nope}: the browser's blocks are {:#if}"],
        ),
        (
            "two elses",
            "{:#if a}x{:else}y{:else}z{/if}<script>let a</script>",
            &["{:else} is not allowed here: a {:#if} takes {:else if …} and then one {:else}"],
        ),
        (
            "an else if in an each",
            "{:#each xs as x}a{:else if b}c{/each}<script>let xs, b</script>",
            &["a {:#each} takes one {:else}, shown when the list is empty"],
        ),
        (
            "media without a query",
            "<p client:media>x</p>",
            &["src/routes/+page.wisp:1:4: `client:media` needs its query"],
        ),
        (
            "a start with a value",
            "<p client:visible=\"x\">x</p>",
            &["`client:visible` takes no value"],
        ),
        (
            "an event without a name",
            "<a on:=\"go\">",
            &["needs an event's name, such as on:click"],
        ),
    ]);
}

#[test]
fn component_tags() {
    page_fails(&[
        (
            "a snippet and a prop of one name",
            "<Card row={r}>{#snippet row()}a{/snippet}</Card>",
            &["src/routes/+page.wisp:1:1: <Card> is given `row` twice"],
        ),
        (
            "two ways to start",
            "<Card client:visible client:idle />",
            &["src/routes/+page.wisp:1:22: <Card> starts one way: `client:idle` is a second"],
        ),
        (
            "a way that does not exist",
            "<Card client:nope />",
            &["`client:nope` is not a way to start: client:load"],
        ),
        (
            "media without a query",
            "<Card client:media />",
            &["`client:media` needs its query"],
        ),
        (
            "media with an empty query",
            "<Card client:media=\"\" />",
            &["`client:media` needs its query"],
        ),
        (
            "a start with a value",
            "<Card client:visible=\"x\" />",
            &["`client:visible` takes no value"],
        ),
        (
            "bind without a name",
            "<Card bind: />",
            &["`bind:` needs a name, such as bind:open"],
        ),
        (
            "on without a value",
            "<Card on:pick />",
            &["`on:pick` needs a value: on:pick=\"…\", with JavaScript in the quotes"],
        ),
        (
            "a bound prop twice",
            "<Card bind:a=\"x\" bind:a=\"y\" />",
            &["<Card> is given `a` twice"],
        ),
        (
            "a prop twice",
            "<Card a=\"x\" a=\"y\" />",
            &["src/routes/+page.wisp:1:13: <Card> is given `a` twice"],
        ),
        (
            "an empty expression",
            "<Card a={} />",
            &["`a={…}` needs an expression, without // comments"],
        ),
        (
            "an expression with a line comment",
            "<Card a={x // y} />",
            &["`a={…}` needs an expression"],
        ),
        (
            "a quoted expression",
            "<Card a=\"{x}\" />",
            &["a quoted prop is plain text; for an expression write a={…}"],
        ),
        (
            "a start on a component the browser renders",
            "<Card a={:x} client:idle />",
            &["<Card> is rendered in the browser here, where its code already runs"],
        ),
        (
            "a server expression on a component the browser renders",
            "<Card a={:x} b={y} />",
            &["so its props are browser values: write b={:…} rather than b={…}"],
        ),
        (
            "a hole in a component's tag",
            "<Card {x + 1} />",
            &["in a component's tag, {name} is name={name}"],
        ),
    ]);
}

#[test]
fn component_files() {
    let page = ("src/routes/+page.wisp", "x");
    fails(&[
        (
            "a lowercase name",
            &[("src/components/my-card.wisp", "x"), page],
            &[
                "src/components/my-card.wisp: a component's file name is its tag",
                "such as Mycard.wisp",
            ],
        ),
        (
            "a duplicate",
            &[
                ("src/components/Card.wisp", "a"),
                ("src/components/sub/Card.wisp", "b"),
                page,
            ],
            &["src/components/sub/Card.wisp: there is already a component `Card`"],
        ),
        (
            "a block of Rust",
            &[
                ("src/components/Card.wisp", "---\nlet a = 1;\n---\nx"),
                page,
            ],
            &["src/components/Card.wisp: a component takes what it shows as {@props …}"],
        ),
        (
            "a broken template",
            &[("src/components/Card.wisp", "\n<p>{#if a}</p>"), page],
            &["src/components/Card.wisp:2:"],
        ),
        (
            "no props listed",
            &[("src/components/Card.wisp", "{@props}\nx"), page],
            &["src/components/Card.wisp:1:1: {@props …} lists the component's props"],
        ),
        (
            "two words for a prop",
            &[("src/components/Card.wisp", "{@props a b}\nx"), page],
            &["`a b` is not a name for a prop"],
        ),
        (
            "a prop with a bad name",
            &[("src/components/Card.wisp", "{@props 1a: u8}\nx"), page],
            &["`1a` is not a name for a prop"],
        ),
        (
            "a prop called children",
            &[
                ("src/components/Card.wisp", "{@props children: u8}\nx"),
                page,
            ],
            &["`children` is a name Wisp uses"],
        ),
        (
            "a prop with an empty default",
            &[("src/components/Card.wisp", "{@props a: u8 = }\nx"), page],
            &["expected `a: Type` or `a: Type = default` in {@props …}"],
        ),
        (
            "a prop with an empty type",
            &[("src/components/Card.wisp", "{@props a: }\nx"), page],
            &["prop `a` needs a type: `a: &str`"],
        ),
        (
            "a prop twice",
            &[
                ("src/components/Card.wisp", "{@props a: u8, a: u8}\nx"),
                page,
            ],
            &["prop `a` is declared twice"],
        ),
        (
            "rest that is not last",
            &[
                (
                    "src/components/Card.wisp",
                    "<p>x</p>\n<script>\n  let { ...r, a } = $props()\n</script>",
                ),
                page,
            ],
            &[
                "src/components/Card.wisp:3:",
                "`...rest` comes last in `$props()`",
            ],
        ),
        (
            "two props runes",
            &[
                (
                    "src/components/Card.wisp",
                    "<p>x</p>\n<script>\n  let { a } = $props()\n  let { b } = $props()\n</script>",
                ),
                page,
            ],
            &[
                "src/components/Card.wisp:4:",
                "a component has one `$props()`",
            ],
        ),
        (
            "a props rune in a page",
            &[(
                "src/routes/+page.wisp",
                "<p>{:x}</p>\n<script>\n  let { x } = $props()\n</script>",
            )],
            &["src/routes/+page.wisp:3:", "`$props()` is for components"],
        ),
        (
            "a props rune in a page that is not one",
            &[(
                "src/routes/+page.wisp",
                "<p>x</p>
<script>
  let { ...r, a } = $props()
</script>",
            )],
            &[
                "src/routes/+page.wisp:3:",
                "`...rest` comes last in `$props()`",
            ],
        ),
        (
            "a prop the rune names and the tag does not",
            &[
                (
                    "src/components/Card.wisp",
                    "{@props a: u8}\n<b>{:a}</b><script>let { a, size } = $props()</script>",
                ),
                ("src/routes/+page.wisp", "<Card a=\"1\" />"),
            ],
            &[
                "src/components/Card.wisp:2:",
                "`size` is not a prop of this component",
                "(it has a)",
            ],
        ),
        (
            "a prop the script declares again",
            &[
                (
                    "src/components/Card.wisp",
                    "{@props n: u8 = 0}\n<b :text=\"n\">{n}</b><script>let n = 1</script>",
                ),
                ("src/routes/+page.wisp", "<Card />"),
            ],
            &[
                "src/components/Card.wisp:2:",
                "`n` is both a server value and a script variable; rename one",
            ],
        ),
    ]);
}

#[test]
fn component_uses() {
    let none = ("src/components/Badge.wisp", "<b>new</b>");
    fails(&[
        (
            "no components at all",
            &[("src/routes/+page.wisp", "\n<Card />")],
            &[
                "src/routes/+page.wisp:2: no component `Card`",
                "there are none yet",
            ],
        ),
        (
            "a component that is not there",
            &[none, ("src/routes/+page.wisp", "<Crad />")],
            &["no component `Crad`", "there are Badge"],
        ),
        (
            "a prop for a component with none",
            &[none, ("src/routes/+page.wisp", "<Badge x=\"1\" />")],
            &["<Badge> has no prop `x`; it takes none"],
        ),
        (
            "children for a component without a slot",
            &[none, ("src/routes/+page.wisp", "<Badge>hi</Badge>")],
            &["<Badge> does not show children"],
        ),
        (
            "a component in the head",
            &[
                none,
                ("src/routes/+page.wisp", "<wisp:head><Badge /></wisp:head>"),
            ],
            &["<Badge> is a component, which cannot go in <wisp:head>"],
        ),
        (
            "a component in a nested block of a layout",
            &[
                none,
                (
                    "src/routes/+layout.wisp",
                    "{#if a}{#each x as y}<Nope />{/each}{/if}{@render children()}",
                ),
                ("src/routes/+page.wisp", "x"),
            ],
            &["src/routes/+layout.wisp:1: no component `Nope`"],
        ),
        (
            "a component in a snippet, a match and an else",
            &[
                none,
                (
                    "src/routes/+page.wisp",
                    "{#snippet s()}<Badge />{/snippet}{#match 1}{:case _}<Badge />{/match}{#if a}x{:else}<Nope />{/if}{#each xs as x}a{:else}<Badge />{/each}",
                ),
            ],
            &["src/routes/+page.wisp:1: no component `Nope`"],
        ),
        (
            "a component in a browser block",
            &[
                none,
                (
                    "src/routes/+page.wisp",
                    "{:#if a}<Nope x={:1} />{/if}<script>let a = 1</script>",
                ),
            ],
            &["no component `Nope`"],
        ),
        (
            "a component in a browser block, and no components",
            &[(
                "src/routes/+page.wisp",
                "{:#if a}<Nope x={:1} />{/if}<script>let a = 1</script>",
            )],
            &["no component `Nope`", "none yet"],
        ),
        (
            "a flag for a typed prop",
            &[
                ("src/components/Card.wisp", "{@props title: &str}\nx"),
                ("src/routes/+page.wisp", "<Card title />"),
            ],
            &["`title` alone means true, but <Card>'s `title` is a `&str`: write title={…}"],
        ),
        (
            "a start for a component without browser code",
            &[none, ("src/routes/+page.wisp", "<Badge client:idle />")],
            &["<Badge> has no browser code, so `client:idle` has nothing to load"],
        ),
        (
            "a component in the browser with server code",
            &[
                (
                    "src/components/Card.wisp",
                    "{@props title: &str}\n<p>{title}</p>",
                ),
                (
                    "src/routes/+page.wisp",
                    "{:#each xs as x}<Card title={:x} />{/each}<script>let xs = []</script>",
                ),
            ],
            &["<Card> is rendered in the browser here, but its markup has server code"],
        ),
        (
            "a prop the browser component lacks",
            &[
                (
                    "src/components/Card.wisp",
                    "{@props title: &str}\n<p>{:title}</p>",
                ),
                ("src/routes/+page.wisp", "<Card title={:1} nope={:2} />"),
            ],
            &["<Card> has no prop `nope`; it takes title"],
        ),
        (
            "a prop the browser component does not take, and it takes none",
            &[
                (
                    "src/components/Tally.wisp",
                    "<button on:click=\"n++\">{:n}</button><script>let n = 0</script>",
                ),
                ("src/routes/+page.wisp", "<Tally nope={:2} />"),
            ],
            &["<Tally> has no prop `nope`; it takes none"],
        ),
        (
            "a bind on a prop that is not bindable",
            &[
                (
                    "src/components/Item.wisp",
                    "{@props label: &str, count: u32 = 0}\n<b>{:label}</b><script>let { label, count } = $props()</script>",
                ),
                (
                    "src/routes/+page.wisp",
                    "<Item label={:\"x\"} bind:count=\"n\" /><script>let n = 0</script>",
                ),
            ],
            &[
                "<Item>'s `count` is not bindable",
                "`let { count = $bindable() } = $props()`",
            ],
        ),
        (
            "a component that renders itself forever",
            &[
                (
                    "src/components/Tree.wisp",
                    "{@props node: &str}\n<p>{:node.name}</p><Tree node={:node} />",
                ),
                ("src/routes/+page.wisp", "<Tree node={:{ name: 'r' }} />"),
            ],
            &[
                "src/components/Tree.wisp:",
                "<Tree> renders itself here with nothing to stop it",
            ],
        ),
    ]);
}
