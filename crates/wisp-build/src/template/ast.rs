//! The parsed template: its nodes, directives, props and errors.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub src: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Index into `Template::chunks`.
    Text(usize),
    /// `{expr}`, HTML-escaped. (`attr={expr}` gets its quotes as text.)
    Expr(Code),
    /// The value of a URL attribute such as `href` starts here, `prefix`
    /// bytes back, and its scheme is decided by an expression. `UrlEnd`
    /// follows where the value ends: a value that would run script there
    /// (`javascript:`) is replaced (`wisp::rt::guard_url`).
    UrlStart {
        prefix: String,
    },
    UrlEnd,
    /// `disabled={cond}`: ` disabled` when `cond` is true, nothing otherwise.
    /// With `class` set it is `class:name={cond}`, a name inside the `class`
    /// value: the space before it is left out when it would come first.
    Bool {
        name: String,
        code: Code,
        class: bool,
    },
    /// `href={expr}`: ` href="…"`, or nothing when `expr` is an `Option`
    /// that is `None`. `url` is set for URL attributes, which are guarded.
    Attr {
        name: String,
        code: Code,
        url: bool,
    },
    /// `{@html expr}`, not escaped.
    Html(Code),
    /// `{@const name = expr}`.
    Const(Code),
    /// `{@render children()}` in a layout.
    Render,
    /// `{#snippet name(params)}…{/snippet}`: markup rendered later by
    /// `{@render name(args)}` or given to a component as a prop. Each
    /// parameter is a Rust `let` pattern, with a type or without.
    Snippet {
        name: String,
        params: Vec<String>,
        body: Vec<Node>,
        line: u32,
    },
    /// `{@render name(args)}`: a snippet of this file above it (`local`),
    /// or a prop holding one. `args` as written, without the parentheses.
    RenderSnippet {
        name: String,
        args: Code,
        local: bool,
    },
    If {
        branches: Vec<(Code, Vec<Node>)>,
        otherwise: Option<Vec<Node>>,
    },
    Each {
        iter: Code,
        pat: String,
        index: Option<String>,
        body: Vec<Node>,
        otherwise: Option<Vec<Node>>,
    },
    Match {
        scrutinee: Code,
        arms: Vec<(Code, Vec<Node>)>,
    },
    /// `{#await future}…{:then v}…{:catch e}…{/await}` in a page: `pending`
    /// goes out with it, a branch with its pattern later (`wisp::rt::defer`).
    Await {
        future: Code,
        pending: Vec<Node>,
        then: Option<(String, Vec<Node>)>,
        catch: Option<(String, Vec<Node>)>,
    },
    /// `<wisp:head>…</wisp:head>`: output goes to the document head.
    Head(Vec<Node>),
    /// `<Card title={x}>…</Card>`: a component from `src/components`, with
    /// its props as written and its children (`None` for `<Card />`).
    Component {
        name: String,
        props: Vec<Prop>,
        children: Option<Vec<Node>>,
        line: u32,
    },
    /// A named field of an action's form (see `Parser::form_defaults`):
    /// `sent`, which reads what was sent as `__k`, when the action refused
    /// it, else `own`. A component, having no `cx`, writes `own` alone, as
    /// does a GET, which is all that is baked.
    Kept {
        name: String,
        sent: Vec<Node>,
        own: Option<Vec<Node>>,
        line: u32,
    },
    /// A kept `<select>`'s choice, `__wisp_sel`, by which its options are
    /// `selected` (see `IS`): what was sent, when the action refused it,
    /// else `own`, the code of its `value={…}`.
    Chosen {
        name: String,
        own: Option<String>,
        line: u32,
    },
    /// ` selected` on an `<option>` whose value (the code of a `&str`) is
    /// its `<select>`'s choice, `__wisp_sel` (see `Chosen`).
    Selected(Code),
    /// What was wrong with field `name`, when the action refused it:
    /// `<small class="problem">…</small>`. After a kept field (`auto`),
    /// unless the file shows it itself: `{cx.problem("x")}` is one.
    Problem {
        name: String,
        line: u32,
        auto: bool,
    },
    /// Where an element's browser directives were, just before its `>`:
    /// its `protocol::GROUP_ATTR` (and `LOOP_ATTR`) for
    /// `Template::groups[group]`.
    Live {
        group: usize,
    },
    /// After a `{:expr}`'s anchor: its first paint, when the server knows
    /// the value (a server value's path), else nothing.
    Hole {
        group: usize,
    },
    /// The name in `<wisp:element this="…">` and its end tag: what the
    /// server knows of `this` (group's `Tag` directive), else
    /// `wisp-element`, which the browser gives its tag.
    Tag {
        group: usize,
    },
    /// A client block or client component: per branch (`{:else}` starts
    /// one), the group of its `<template>`'s directive and the template's
    /// content, which the browser copies. The server also paints the copies
    /// after each template when it knows the values (see `codegen`).
    Client(Vec<(usize, Vec<Node>)>),
}

/// A directive: browser code on an element, such as `on:click="…"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    pub kind: Dir,
    /// The event, attribute, class, style property, transition or `use`
    /// function; `value`, `checked` or `this` for `bind:`; for `each`, the
    /// item's name.
    pub name: String,
    /// An event's modifiers, as written; for `each`, the index's name.
    pub mods: Vec<String>,
    /// The JavaScript, when there is a value.
    pub value: Option<Code>,
    /// A client `each`'s key: `{:#each todos as todo (todo.id)}`.
    pub key: Option<Code>,
    /// A client component's props (`Dir::Comp`).
    pub props: Vec<Prop>,
    /// Where the directive's name is.
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    On,
    Bind,
    Attr,
    Text,
    Class,
    Style,
    Transition,
    Use,
    /// `<template each="item, i in list">`
    Each,
    /// `<template if="cond">`
    If,
    /// `{:expr}` in text, on its `<template>` anchor.
    Hole,
    /// `animate:flip` on an element of a keyed client `each`.
    Animate,
    /// A component the browser renders: `name` is the component.
    Comp,
    /// `{:...attrs}` in a tag: an object's keys as attributes.
    Spread,
    /// `{:#key expr}`: its content drawn afresh when `expr` changes.
    Key,
    /// A `{#snippet}` given to a component the browser draws: `name` is the
    /// prop it is given as, `mods` its parameters, and its body is the block's.
    Snip,
    /// `{:#await promise}`: one copy, whose `__aw` says how it went.
    Await,
    /// `{:#try}`: one copy, whose `__tr` holds what failed in it.
    Try,
    /// `<wisp:window>`, `<wisp:document>`, `<wisp:body>`: where the other
    /// directives go (`name`).
    At,
    /// `client:visible` (`name` is `v`, `i`, `x`, `n` or `m(query)`) on an
    /// element: it and what is inside it start then.
    Wait,
    /// `this="tag"` on `<wisp:element>`: the element's tag.
    Tag,
}

/// The directives of one element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub directives: Vec<Directive>,
    /// The names of the client `<template each>`s around it.
    pub locals: Vec<String>,
    /// Inside a client `<template>`, whose copies the browser binds.
    pub nested: bool,
    pub line: u32,
}

/// The file's client script: its bare `<script>`, as written (or its
/// `<script lang="ts">`, its types blanked out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub src: String,
    /// Where its text starts.
    pub line: u32,
    pub col: u32,
    /// A `lang="ts"` script's text as written, for `wisp check --types`.
    pub ts: Option<String>,
}

/// A prop given to a component: `name={expr}`, `name="text"` or `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub name: String,
    pub value: PropValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropValue {
    Expr(Code),
    Text(String),
    /// The name alone: `true`.
    Flag,
    /// `name={:js}`: a browser value.
    Live(Code),
    /// `bind:name="target"`: the prop and a script variable, both ways.
    Bind(Code),
    /// `on:name="handler"`: called when the component does `emit('name', x)`.
    On(Code),
    /// A snippet of this file: `{row}`, `name={row}`, or a `{#snippet}`
    /// written as a child of the component. `arity` is its parameter count.
    Snippet {
        name: String,
        arity: usize,
    },
}

/// The tag of `{@element "x-card"}`: a valid custom element name (lower
/// case, a letter first, a `-` in it, not one HTML keeps).
pub(super) fn element_name(arg: &str) -> Result<String, String> {
    let arg = arg.trim();
    let Some(tag) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) else {
        return Err(format!(
            "{{@element \"x-card\"}} names its tag in quotes, found `{arg}`"
        ));
    };
    let ok = tag.starts_with(|c: char| c.is_ascii_lowercase())
        && tag.contains('-')
        && (tag.bytes()).all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')
        })
        && !matches!(
            tag,
            "annotation-xml"
                | "color-profile"
                | "font-face"
                | "font-face-src"
                | "font-face-uri"
                | "font-face-format"
                | "font-face-name"
                | "missing-glyph"
        );
    if !ok {
        return Err(format!(
            "`{tag}` is not a custom element name: lower case, a letter first and a `-` in it, as `x-card`"
        ));
    }
    Ok(tag.to_string())
}

/// A prop a component declares: `{@props title: &str, size: u8 = 2}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropDecl {
    pub name: String,
    pub ty: String,
    pub default: Option<String>,
}

#[derive(Debug)]
pub struct Template {
    pub nodes: Vec<Node>,
    pub chunks: Vec<String>,
    /// Hash of everything except static text. Equal shape ⇒ hot-swappable.
    pub shape: u64,
    pub uses_children: bool,
    /// `{@props …}`, which only a component has, and its line.
    pub props: Option<(Vec<PropDecl>, u32)>,
    /// `{@element "x-card"}`: the custom element a component is also built
    /// as, and its line.
    pub element: Option<(String, u32)>,
    /// Browser code: the client script and every element's directives.
    pub script: Option<Script>,
    pub groups: Vec<Group>,
    /// Its bare `<style>`s, scoped: CSS for `/_app/app.css`.
    pub style: Option<String>,
    /// Its accessibility warnings.
    pub lints: Vec<crate::a11y::Lint>,
    /// `'sha256-…'` of each inline script that runs (see `csp`).
    pub hashes: Vec<String>,
    /// A page with `const SSR: bool = false;`: the group of the client
    /// block its markup (but its `<head>`) is wrapped in, which the server
    /// never paints, so the browser draws the page from its data.
    pub drawn: Option<usize>,
    /// Says `{@flash}`: the pages it draws take the flash message first.
    pub flash: bool,
}

impl Template {
    /// Has code for the browser, so each render is an instance of a module.
    pub fn is_live(&self) -> bool {
        self.script.is_some() || !self.groups.is_empty()
    }

    /// `n` is a `<title>` (the head of its own the parser gives one).
    pub fn is_title(&self, n: &Node) -> bool {
        matches!(n, Node::Head(b) if matches!(b.first(), Some(&Node::Text(i)) if self.chunks[i].starts_with("<title")))
    }

    /// Writes a `<title>`, at the top level or in its `<head>`. Of a page
    /// and its layouts the innermost that does writes the one title.
    pub fn has_title(&self) -> bool {
        self.nodes.iter().any(|n| match n {
            Node::Head(b) => self.is_title(n) || b.iter().any(|n| self.is_title(n)),
            _ => false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: u32,
    pub col: u32,
    pub msg: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}
