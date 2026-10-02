//! The `.wisp` template language: HTML with Rust holes.
//!
//! Parsing strips comments, collapses indentation and produces a node tree
//! whose static text lives in `Template::chunks`. Every child list alternates
//! `Text, X, Text, X, …, Text` (texts may be empty). Because of that, editing
//! only static text never changes the template's *shape* (the nodes, their
//! code and their HTML context), and dev builds can hot-swap text without a
//! compile. Anything that could change what the generated Rust does is part
//! of the shape.
//!
//! Browser code is kept apart from the HTML: the bare `<script>` and each
//! element's directives (`on:click="…"`, `:hidden="…"`, …) leave the text,
//! and a `Node::Live` marks where the element's `data-w` goes. Their
//! JavaScript is part of the shape, since it is compiled into the binary.

use crate::contexts::{Held, Scheme, holds_script, is_url_attr, scheme};
use crate::protocol::{
    ISLAND_IDLE, ISLAND_INTERACTION, ISLAND_MEDIA, ISLAND_NONE, ISLAND_VISIBLE, ON_FLAGS,
};
use crate::rules::Field;
use crate::ty::{is_ident, is_word};
use crate::{fnv1a, js};

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

/// The file's client script: its bare `<script>`, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub src: String,
    /// Where its text starts.
    pub line: u32,
    pub col: u32,
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
    /// Browser code: the client script and every element's directives.
    pub script: Option<Script>,
    pub groups: Vec<Group>,
}

impl Template {
    /// Has code for the browser, so each render is an instance of a module.
    pub fn is_live(&self) -> bool {
        self.script.is_some() || !self.groups.is_empty()
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

pub fn parse(src: &str) -> Result<Template, Error> {
    parse_with(src, &[])
}

/// A page's template, whose action forms' `fields` get the attributes the
/// browser checks them by (`rules::Native`).
pub fn parse_with(src: &str, fields: &[Field]) -> Result<Template, Error> {
    let mut p = Parser {
        src,
        fields,
        b: src.as_bytes(),
        i: 0,
        ctx: Ctx::Text,
        tag: String::new(),
        tag_pos: 0,
        closing: false,
        typed: false,
        attr: String::new(),
        last: b' ',
        value_start: 0,
        value_events: false,
        url_guard: false,
        preserve: 0,
        text: String::new(),
        chunks: Vec::new(),
        root: Vec::new(),
        frames: Vec::new(),
        opened: Vec::new(),
        uses_children: false,
        props: None,
        line_starts: std::iter::once(0)
            .chain(src.match_indices('\n').map(|(i, _)| i + 1))
            .collect(),
        tag_text: 0,
        tag_frames: 0,
        tag_attrs: false,
        directives: Vec::new(),
        tag_classes: Vec::new(),
        templates: Vec::new(),
        script: None,
        groups: Vec::new(),
        end: src.len(),
        snippets: Vec::new(),
        rendering: Vec::new(),
        elements: Vec::new(),
        tag_seen: Vec::new(),
        forms: Vec::new(),
        svg: 0,
        auto_head: false,
        button_form: false,
        problem: None,
        shown: Vec::new(),
        value_at: None,
        tag_nodes: 0,
        keep: None,
    };
    p.run()?;
    if !p.shown.is_empty() {
        drop_shown(&mut p.root, &mut p.chunks, &p.shown);
    }

    // Trim the template as a whole; inner whitespace was already collapsed.
    if let Some(&Node::Text(first)) = p.root.first() {
        p.chunks[first] = p.chunks[first].trim_start().to_string();
    }
    if let Some(&Node::Text(last)) = p.root.last() {
        p.chunks[last] = p.chunks[last].trim_end().to_string();
    }

    let mut h = Vec::new();
    shape(&p.root, &mut h);
    for d in p.props.iter().flat_map(|(ds, _)| ds) {
        h.extend_from_slice(
            format!(
                "P{}\0{}\0{}\0",
                d.name,
                d.ty,
                d.default.as_deref().unwrap_or("")
            )
            .as_bytes(),
        );
    }
    // Browser code is compiled into the binary as a module, so changing it
    // takes a build.
    if let Some(s) = &p.script {
        h.extend_from_slice(b"S");
        h.extend_from_slice(s.src.as_bytes());
        h.push(0);
    }
    for g in &p.groups {
        h.extend_from_slice(format!("G{}\0{}\0", g.nested, g.locals.join(",")).as_bytes());
        for d in &g.directives {
            h.extend_from_slice(
                format!("{:?}\0{}\0{}\0", d.kind, d.name, d.mods.join(".")).as_bytes(),
            );
            h.extend_from_slice(
                d.value
                    .as_ref()
                    .map_or("\u{1}", |v| v.src.as_str())
                    .as_bytes(),
            );
            h.push(0);
            h.extend_from_slice(
                d.key
                    .as_ref()
                    .map_or("\u{1}", |v| v.src.as_str())
                    .as_bytes(),
            );
            h.push(0);
            prop_shape(&d.props, &mut h);
        }
    }
    // A component the browser can render has its markup compiled into its
    // module too, so its text is part of the shape. (One without props is
    // not told from a page here: its text hot-swaps on the server only.)
    if p.props.is_some() && client_renderable(&p.root) {
        for c in &p.chunks {
            h.extend_from_slice(c.as_bytes());
            h.push(0);
        }
    }
    Ok(Template {
        shape: fnv1a(&h),
        nodes: p.root,
        chunks: p.chunks,
        uses_children: p.uses_children,
        props: p.props,
        script: p.script,
        groups: p.groups,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Text,
    Tag,
    Quoted(u8),
}

/// Where in the HTML the parser is, as far as a hole cares: text, a tag,
/// right after `name=`, or a quoted value.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Place {
    ctx: Ctx,
    value_next: bool,
}

impl Place {
    fn describe(self) -> &'static str {
        match self.ctx {
            Ctx::Text => "in text",
            Ctx::Tag if self.value_next => "right after an attribute's `=`",
            Ctx::Tag => "inside a tag",
            Ctx::Quoted(_) => "inside a quoted attribute value",
        }
    }
}

/// Where a block began. Each of its branches must begin there too, and the
/// block must end there: otherwise one branch could leave the page inside a
/// tag that another never opened, and a later `{x}` would be escaped for
/// the wrong place.
struct Opened {
    place: Place,
    last: u8,
    attr: String,
}

enum Frame {
    If {
        pos: usize,
        branches: Vec<(Code, Vec<Node>)>,
        otherwise: Option<Vec<Node>>,
    },
    Each {
        pos: usize,
        iter: Code,
        pat: String,
        index: Option<String>,
        body: Vec<Node>,
        otherwise: Option<Vec<Node>>,
    },
    Match {
        pos: usize,
        scrutinee: Code,
        arms: Vec<(Code, Vec<Node>)>,
    },
    Head {
        pos: usize,
        body: Vec<Node>,
    },
    Component {
        pos: usize,
        name: String,
        props: Vec<Prop>,
        body: Vec<Node>,
    },
    /// `start` is where its body's source begins.
    Snippet {
        pos: usize,
        name: String,
        params: Vec<String>,
        start: usize,
        body: Vec<Node>,
    },
    /// `{:#if}`, `{:#each}` or a client component with children: `kind` is
    /// `if`, `each` or `comp`. `name` is the component's; `conds`, an if's
    /// conditions so far, or an each's list (so `{:else}` can say "none").
    /// `branches` are the ones before the current, whose group is `group`.
    Client {
        pos: usize,
        kind: &'static str,
        name: String,
        conds: Vec<String>,
        has_else: bool,
        group: usize,
        branches: Vec<(usize, Vec<Node>)>,
        body: Vec<Node>,
    },
}

impl Frame {
    /// How the block is written, and where it opened.
    fn describe(&self) -> (String, usize) {
        match self {
            Frame::If { pos, .. } => ("{#if}".into(), *pos),
            Frame::Each { pos, .. } => ("{#each}".into(), *pos),
            Frame::Match { pos, .. } => ("{#match}".into(), *pos),
            Frame::Head { pos, .. } => ("<wisp:head>".into(), *pos),
            Frame::Component { pos, name, .. } => (format!("<{name}>"), *pos),
            Frame::Snippet { pos, .. } => ("{#snippet}".into(), *pos),
            Frame::Client {
                pos,
                kind: "comp",
                name,
                ..
            } => (format!("<{name}>"), *pos),
            Frame::Client { pos, kind, .. } => (format!("{{:#{}}}", block_of(kind)), *pos),
        }
    }
}

struct Parser<'a> {
    src: &'a str,
    /// The fields of the page's actions the browser can check.
    fields: &'a [Field],
    b: &'a [u8],
    i: usize,
    ctx: Ctx,
    /// Lowercased name of the tag being scanned, its position, and whether it is `</…>`.
    tag: String,
    tag_pos: usize,
    closing: bool,
    /// The tag has a `type` or `src` attribute (see `tag_close`).
    typed: bool,
    /// Lowercased name of the attribute being scanned inside a tag.
    attr: String,
    /// Last significant byte inside a tag; `=` means a value comes next.
    last: u8,
    /// Where the current attribute value's text starts in `text`, and
    /// whether a hole or block has appeared in it yet (the first one flushes
    /// `text`, so `value_start` is good until then).
    value_start: usize,
    value_events: bool,
    /// A `UrlStart` is open and needs its `UrlEnd` where the value ends.
    url_guard: bool,
    /// Depth of `<pre>`/`<textarea>`, where whitespace is kept verbatim.
    preserve: u32,
    text: String,
    chunks: Vec<String>,
    root: Vec<Node>,
    frames: Vec<Frame>,
    /// One per frame.
    opened: Vec<Opened>,
    uses_children: bool,
    props: Option<(Vec<PropDecl>, u32)>,
    line_starts: Vec<usize>,
    /// Where the tag being scanned starts in `text`, how many blocks were
    /// open at its start, and whether it has any attribute (or hole).
    tag_text: usize,
    tag_frames: usize,
    tag_attrs: bool,
    /// The directives of the tag being scanned.
    directives: Vec<Directive>,
    /// The `class:name={cond}` of the tag being scanned, until they are
    /// written into its `class` attribute.
    tag_classes: Vec<(String, Code)>,
    /// One per open `<template>`: the names a client `<template each|if>`
    /// gives the elements inside it, `None` for a plain one.
    templates: Vec<Option<Vec<String>>>,
    script: Option<Script>,
    groups: Vec<Group>,
    /// Where scanning stops: the end, or the end of a snippet's body that
    /// `{:@render}` is copying.
    end: usize,
    /// The snippets defined so far that are still in scope.
    snippets: Vec<Snip>,
    /// The snippets `{:@render}` is copying, innermost last.
    rendering: Vec<String>,
    /// The groups of the `<wisp:element>`s open, innermost last.
    elements: Vec<usize>,
    /// The attributes of the tag being scanned, each with its value when
    /// that is plain text.
    tag_seen: Vec<(String, Option<String>)>,
    /// One per open `<form>`: the action it posts to (`?/name`, `default`
    /// for a `method="post"` one), `""` when that is not plain, `None` for
    /// one that posts to none.
    forms: Vec<Option<String>>,
    /// Depth of `<svg>`, whose `<title>` is its own.
    svg: u32,
    /// A top-level `<title>` is open, in the head it opened.
    auto_head: bool,
    /// A `<button action="?/name">` is open, in the form it opened.
    button_form: bool,
    /// The input just scanned, whose problem goes after it (`Node::Problem`).
    problem: Option<String>,
    /// The fields whose problem the file shows itself, `{cx.problem("x")}`:
    /// none goes after their inputs.
    shown: Vec<String>,
    /// The tag's `value` attribute as text: the chunk count when it was
    /// written, and where it starts and ends in `text` (see `own_value`).
    value_at: Option<(usize, usize, usize)>,
    /// The length of the current list when the tag being scanned began:
    /// the nodes of its attributes are after it.
    tag_nodes: usize,
    /// The `<textarea>` or `<select>` of an action's form that is open.
    keep: Option<Keep>,
}

/// A `<textarea>` or `<select>` of an action's form, till its end tag:
/// what was sent goes back in it, and its problem after it.
struct Keep {
    name: String,
    textarea: bool,
    /// Where its content begins: the frame depth and the list's length.
    frames: usize,
    at: usize,
}

/// A snippet defined above: its body's source, and the depth of the list
/// it is in (it goes out of scope with that list).
struct Snip {
    name: String,
    params: Vec<String>,
    body: std::ops::Range<usize>,
    depth: usize,
}

impl Parser<'_> {
    fn run(&mut self) -> Result<(), Error> {
        self.scan()?;
        if self.ctx != Ctx::Text {
            return Err(self.err(self.tag_pos, format!("unclosed <{}> tag", self.tag)));
        }
        self.flush()?;
        if let Some(f) = self.frames.last() {
            let (what, pos) = f.describe();
            return Err(self.err(pos, format!("{what} is never closed")));
        }
        Ok(())
    }

    fn scan(&mut self) -> Result<(), Error> {
        while self.i < self.end {
            let c = self.b[self.i];
            match self.ctx {
                Ctx::Text => match c {
                    b'{' => self.hole()?,
                    b'<' => self.tag_open()?,
                    _ if is_ws(c) => self.whitespace(),
                    _ => self.copy_until(|c| c == b'{' || c == b'<' || is_ws(c)),
                },
                Ctx::Tag => match c {
                    b'{' => self.hole()?,
                    b'>' => self.tag_close()?,
                    b'"' | b'\'' => {
                        self.push_byte(c);
                        self.ctx = Ctx::Quoted(c);
                        self.value_start = self.text.len();
                        self.value_events = false;
                    }
                    b'=' | b'/' => {
                        self.push_byte(c);
                        self.last = c;
                    }
                    _ if is_ws(c) => {
                        self.whitespace();
                        // `name= value` is still name's value.
                        if self.last != b'=' {
                            self.last = b' ';
                        }
                    }
                    _ => {
                        let start = self.i;
                        self.copy_until(|c| {
                            matches!(c, b'{' | b'>' | b'"' | b'\'' | b'=' | b'/') || is_ws(c)
                        });
                        if self.last != b'=' {
                            self.tag_attrs = true;
                            let raw = &self.src[start..self.i];
                            if !self.closing && is_directive(raw, &self.tag) {
                                self.directive(start)?;
                                self.last = b'a';
                                continue;
                            }
                            self.attr = raw.to_ascii_lowercase();
                            if self.button_form && self.tag == "button" && self.attr == "action" {
                                self.text.insert_str(self.text.len() - raw.len(), "form");
                                self.attr = "formaction".into();
                            }
                            self.typed |= matches!(self.attr.as_str(), "type" | "src" | "nomodule");
                            self.tag_seen.push((self.attr.clone(), None));
                            if self.attr == "value" {
                                let at = self.text.len() - raw.len();
                                self.value_at = Some((self.chunks.len(), at, at));
                            }
                        } else if let Some(a) = self.tag_seen.last_mut() {
                            a.1 = Some(self.src[start..self.i].to_string());
                            self.value_ends();
                        }
                        self.last = b'a';
                    }
                },
                Ctx::Quoted(q) => match c {
                    b'{' => self.hole()?,
                    _ if c == q => {
                        if !self.value_events
                            && let Some(a) = self.tag_seen.last_mut()
                        {
                            a.1 = Some(self.text[self.value_start..].to_string());
                        }
                        self.end_value(self.i)?;
                        if self.attr == "class" {
                            self.write_classes()?;
                        }
                        self.push_byte(c);
                        self.value_ends();
                        self.ctx = Ctx::Tag;
                        self.last = b'a';
                    }
                    _ => self.copy_until(|c| c == b'{' || c == q),
                },
            }
        }
        Ok(())
    }

    // ---- text -------------------------------------------------------------

    fn push_byte(&mut self, c: u8) {
        debug_assert!(c.is_ascii());
        self.text.push(c as char);
        self.i += 1;
    }

    /// Copies source bytes to the text buffer until `stop` matches. Stop bytes
    /// are all ASCII, so the slice boundaries are always valid UTF-8.
    fn copy_until(&mut self, stop: impl Fn(u8) -> bool) {
        let start = self.i;
        while self.i < self.b.len() && !stop(self.b[self.i]) {
            self.i += 1;
        }
        self.text.push_str(&self.src[start..self.i]);
    }

    /// A whitespace run containing a newline becomes one newline; others stay.
    /// This removes indentation without changing how the HTML renders.
    fn whitespace(&mut self) {
        let start = self.i;
        while self.i < self.b.len() && is_ws(self.b[self.i]) {
            self.i += 1;
        }
        let run = &self.src[start..self.i];
        if self.preserve == 0 && run.contains('\n') {
            self.text.push('\n');
        } else {
            self.text.push_str(run);
        }
    }

    /// Moves the text buffer into the current list as a `Text` node. Called
    /// before every structural event, which is what keeps lists alternating.
    fn flush(&mut self) -> Result<(), Error> {
        // A quoted value cut here has its start in an earlier chunk, so
        // `value_start` is no good after it: the cut counts as an event in
        // the value (a branch or block end in one that began in the block).
        if matches!(self.ctx, Ctx::Quoted(_)) {
            self.in_value(self.i)?;
        }
        if let Some(Frame::Match { pos, arms, .. }) = self.frames.last()
            && arms.is_empty()
        {
            if !self.text.trim().is_empty() {
                return Err(self.err(*pos, "only {:case …} may follow {#match …}".into()));
            }
            self.text.clear();
            return Ok(());
        }
        let idx = self.chunks.len();
        self.chunks.push(std::mem::take(&mut self.text));
        self.list().push(Node::Text(idx));
        Ok(())
    }

    fn list(&mut self) -> &mut Vec<Node> {
        match self.frames.last_mut() {
            None => &mut self.root,
            Some(Frame::If {
                branches,
                otherwise,
                ..
            }) => match otherwise {
                Some(o) => o,
                None => &mut branches.last_mut().expect("if frame always has a branch").1,
            },
            Some(Frame::Each {
                body, otherwise, ..
            }) => match otherwise {
                Some(o) => o,
                None => body,
            },
            Some(Frame::Match { arms, .. }) => {
                &mut arms
                    .last_mut()
                    .expect("flush rejects nodes before the first case")
                    .1
            }
            Some(
                Frame::Head { body, .. }
                | Frame::Component { body, .. }
                | Frame::Snippet { body, .. }
                | Frame::Client { body, .. },
            ) => body,
        }
    }

    /// Starts a node or block at `pos`: flushes pending text into the current
    /// list. A block's own node is pushed at its close *without* another
    /// flush, since everything after its open went into the block.
    fn begin(&mut self, pos: usize) -> Result<(), Error> {
        if let Some(Frame::Match { arms, .. }) = self.frames.last()
            && arms.is_empty()
        {
            return Err(self.err(pos, "only {:case …} may follow {#match …}".into()));
        }
        self.flush()
    }

    fn push_node(&mut self, pos: usize, node: Node) -> Result<(), Error> {
        self.begin(pos)?;
        self.list().push(node);
        Ok(())
    }

    fn place(&self) -> Place {
        Place {
            ctx: self.ctx,
            value_next: self.ctx == Ctx::Tag && self.last == b'=',
        }
    }

    fn open(&mut self, frame: Frame) {
        self.opened.push(Opened {
            place: self.place(),
            last: self.last,
            attr: self.attr.clone(),
        });
        self.frames.push(frame);
    }

    /// A branch (`{:else}`, `{:case}`) or the end of the innermost block is
    /// at `pos`: it must be where the block began.
    fn same_place(&self, pos: usize, what: &str) -> Result<(), Error> {
        let (Some(o), Some(f)) = (self.opened.last(), self.frames.last()) else {
            return Ok(());
        };
        if o.place == self.place() {
            return Ok(());
        }
        let (name, at) = f.describe();
        Err(self.err(
            pos,
            format!(
                "{what} is {} but its {name} (line {}) is {}. A block must begin and end in the same place, \
                 such as both in text or both inside one tag, so that every branch leaves the page in the same place.",
                self.place().describe(),
                self.line_of(at),
                o.place.describe()
            ),
        ))
    }

    /// A new branch of the innermost block starts where the block did.
    fn restart(&mut self) {
        if let Some(o) = self.opened.last() {
            self.last = o.last;
            self.attr.clone_from(&o.attr);
        }
    }

    /// Closes the innermost block, which the caller checked exists: flushes
    /// its last text and returns the frame.
    fn end(&mut self, pos: usize) -> Result<Frame, Error> {
        self.flush()?;
        let (name, _) = self.frames.last().expect("caller checked").describe();
        self.same_place(pos, &format!("the end of {name}"))?;
        self.opened.pop();
        let frame = self.frames.pop().expect("caller checked");
        let depth = self.frames.len();
        self.snippets.retain(|s| s.depth <= depth);
        Ok(frame)
    }

    /// `{/kw}` or `</wisp:head>` with no block of that kind open.
    fn unexpected_close(&self, pos: usize, what: &str) -> Error {
        match self.frames.last() {
            Some(f) => {
                let (name, at) = f.describe();
                self.err(pos, format!("{what} does not match the {name} still open from line {}; close that first", self.line_of(at)))
            }
            None => self.err(pos, format!("{what} has no block to close")),
        }
    }

    /// A hole or a block is about to start at `pos`, inside an attribute
    /// value (quoted, or right after `name=`). The first one in a URL
    /// attribute's value decides whether the value needs guarding, from the
    /// static text before it.
    fn in_value(&mut self, pos: usize) -> Result<(), Error> {
        if self.value_events {
            return Ok(());
        }
        self.value_events = true;
        if !is_url_attr(&self.attr) {
            return Ok(());
        }
        let prefix = self.text[self.value_start..].to_string();
        match scheme(&prefix) {
            Scheme::Fixed => Ok(()),
            Scheme::Script => Err(self.err(
                pos,
                format!("no expressions in a `{}` that runs script; put data in data-* attributes and read them from a <script>", self.attr),
            )),
            Scheme::Encoded => Err(self.err(
                pos,
                format!("`{}` has a character reference (`&...;`) before its scheme and an expression; write the URL's start plainly", self.attr),
            )),
            Scheme::Open => {
                self.push_node(pos, Node::UrlStart { prefix })?;
                self.url_guard = true;
                Ok(())
            }
        }
    }

    /// The attribute value ends at `pos`: a guarded one gets its `UrlEnd`.
    fn end_value(&mut self, pos: usize) -> Result<(), Error> {
        if std::mem::take(&mut self.url_guard) {
            self.push_node(pos, Node::UrlEnd)?;
        }
        Ok(())
    }

    // ---- tags -------------------------------------------------------------

    fn tag_open(&mut self) -> Result<(), Error> {
        let start = self.i;
        let rest = &self.b[start + 1..];
        if rest.starts_with(b"!--") {
            // Comments are dropped, holes inside them included. `<!-->` and
            // `<!--->` are whole (empty) comments, as browsers read them.
            let body = &self.src[start + 4..];
            let n = match body.find("-->") {
                _ if body.starts_with('>') => 1,
                _ if body.starts_with("->") => 2,
                Some(n) => n + 3,
                None => return Err(self.err(start, "unclosed <!-- comment".into())),
            };
            self.i = start + 4 + n;
            return Ok(());
        }
        let closing = rest.first() == Some(&b'/');
        let name_start = start + 1 + closing as usize;
        let first = self.b.get(name_start).copied().unwrap_or(0);
        if first == b'{' {
            return Err(self.err(
                start,
                "a tag's name cannot be an expression; choose between tags with {#if}".into(),
            ));
        }
        if !(first.is_ascii_alphabetic() || (!closing && first == b'!')) {
            // A literal '<' in text, like "a < b". Written as `&lt;` so that
            // nothing after it, a hole's value included, can make it a tag.
            self.text.push_str("&lt;");
            self.i += 1;
            return Ok(());
        }
        let mut end = name_start + 1;
        while end < self.b.len()
            && (self.b[end].is_ascii_alphanumeric() || matches!(self.b[end], b'-' | b':' | b'_'))
        {
            end += 1;
        }
        // `<Card>` is a component; `<DIV>`, all capitals, is still HTML.
        let raw_name = &self.src[name_start..end];
        if is_component_name(raw_name) {
            return self.component(start, end, raw_name.to_string(), closing);
        }
        let name = raw_name.to_ascii_lowercase();

        // `<slot />` is `{@render children()}`, as in Vue.
        if name == "slot" && !closing {
            let rest = self.src[end..].trim_start();
            if rest.starts_with("/>") {
                self.uses_children = true;
                self.push_node(start, Node::Render)?;
                self.i = self.src.len() - rest.len() + 2;
                return Ok(());
            }
        }
        // A `<title>` at the top level goes in the head, as it would with
        // `<head>` around it.
        if name == "title" && !closing && self.svg == 0 && self.frames.is_empty() {
            self.begin(start)?;
            self.open(Frame::Head {
                pos: start,
                body: Vec::new(),
            });
            self.auto_head = true;
        }

        // `<button action="?/remove&id={id}">` outside a form is a form of
        // its own, as small as a link: its action is the button's
        // `formaction`.
        if name == "button"
            && !closing
            && self.forms.is_empty()
            && self
                .attr_prefix(start, "action")
                .is_some_and(|v| v.starts_with("?/"))
        {
            self.text.push_str("<form method=\"post\">");
            self.button_form = true;
        }

        // A page is inside `<body>`, so its `<head>` can only mean what goes
        // in the document's: the same as `<wisp:head>`.
        if name == "wisp:head" || name == "head" {
            let mut j = end;
            while j < self.b.len() && is_ws(self.b[j]) {
                j += 1;
            }
            if self.b.get(j) != Some(&b'>') {
                return Err(self.err(start, "<wisp:head> takes no attributes".into()));
            }
            self.i = j + 1;
            if closing {
                // The head a top-level `<title>` opened closes with it.
                if !matches!(self.frames.last(), Some(Frame::Head { .. })) || self.auto_head {
                    return Err(self.unexpected_close(start, "</wisp:head>"));
                }
                if let Frame::Head { body, .. } = self.end(start)? {
                    self.list().push(Node::Head(body));
                }
            } else {
                if self.frames.iter().any(|f| matches!(f, Frame::Head { .. })) {
                    return Err(self.err(start, "<wisp:head> cannot be nested".into()));
                }
                self.begin(start)?;
                self.open(Frame::Head {
                    pos: start,
                    body: Vec::new(),
                });
            }
            return Ok(());
        }

        if closing && matches!(name.as_str(), "textarea" | "select") {
            self.end_keep(start, name == "textarea")?;
        }
        // (Right after `{#match}`, where no tag may be, there is no list.)
        self.tag_nodes = match self.frames.last() {
            Some(Frame::Match { arms, .. }) if arms.is_empty() => 0,
            _ => self.list().len(),
        };
        self.tag_text = self.text.len();
        self.tag_frames = self.frames.len();
        self.tag_attrs = false;
        self.tag_seen.clear();
        self.value_at = None;
        self.directives.clear();
        self.tag_classes = if closing {
            Vec::new()
        } else {
            self.server_classes(end)
        };
        // `<wisp:window on:resize="…" />` and the like: a `<template>` whose
        // directives go on the window; `<wisp:element this="tag">`, an
        // element whose tag the browser sets.
        let target = name
            .strip_prefix("wisp:")
            .filter(|t| matches!(*t, "window" | "document" | "body"));
        if let Some(t) = target {
            if closing {
                return Err(self.err(start, format!("<wisp:{t}> closes itself: <wisp:{t} … />")));
            }
            self.text.push_str("<template");
            self.directives.push(Directive {
                kind: Dir::At,
                name: t.into(),
                mods: Vec::new(),
                value: None,
                key: None,
                props: Vec::new(),
                line: self.line_of(start),
                col: self.col_of(start),
            });
        } else if name == "wisp:element" {
            // Its group is taken now, for the name here and in its end tag.
            self.text.push_str(if closing { "</" } else { "<" });
            let group = if closing {
                self.elements.pop().ok_or_else(|| {
                    self.err(
                        start,
                        "</wisp:element> has no <wisp:element> to close".into(),
                    )
                })?
            } else {
                let g = self.group(Vec::new(), self.line_of(start));
                self.elements.push(g);
                g
            };
            self.push_node(start, Node::Tag { group })?;
            self.tag_text = self.text.len();
        } else if name.starts_with("wisp:") {
            return Err(self.err(start, format!("<{name}> is not a Wisp element: there are <wisp:head>, <wisp:window>, <wisp:document>, <wisp:body> and <wisp:element>")));
        } else {
            self.text.push_str(&self.src[start..end]);
        }
        self.i = end;
        self.ctx = Ctx::Tag;
        self.tag = name;
        self.tag_pos = start;
        self.closing = closing;
        self.attr.clear();
        self.typed = false;
        self.last = b' ';
        if closing && matches!(self.tag.as_str(), "pre" | "textarea") {
            self.preserve = self.preserve.saturating_sub(1);
        }
        Ok(())
    }

    /// `<Card …>`, `<Card … />` or `</Card>`, whose name ends at `name_end`.
    /// A component's tag holds props, not HTML attributes: `name={expr}`,
    /// `name="text"` or `name` alone for `true`.
    fn component(
        &mut self,
        start: usize,
        name_end: usize,
        name: String,
        closing: bool,
    ) -> Result<(), Error> {
        let b = self.b;
        let skip_ws = |mut j: usize| {
            while j < b.len() && is_ws(b[j]) {
                j += 1;
            }
            j
        };
        let mut j = skip_ws(name_end);
        if closing {
            if b.get(j) != Some(&b'>') {
                return Err(self.err(start, format!("</{name}> takes nothing but its name")));
            }
            self.i = j + 1;
            if matches!(self.frames.last(), Some(Frame::Client { kind: "comp", name: open, .. }) if *open == name)
            {
                return self.client_close(start);
            }
            if !matches!(self.frames.last(), Some(Frame::Component { name: open, .. }) if *open == name)
            {
                return Err(self.unexpected_close(start, &format!("</{name}>")));
            }
            if let Frame::Component {
                pos,
                name,
                mut props,
                body,
            } = self.end(start)?
            {
                // A `{#snippet}` among the children is the prop of its name.
                for n in &body {
                    if let Node::Snippet {
                        name: s, params, ..
                    } = n
                    {
                        if props.iter().any(|p| p.name == *s) {
                            return Err(self.err(pos, format!("<{name}> is given `{s}` twice")));
                        }
                        props.push(Prop {
                            name: s.clone(),
                            value: PropValue::Snippet {
                                name: s.clone(),
                                arity: params.len(),
                            },
                        });
                    }
                }
                let line = self.line_of(pos);
                self.list().push(Node::Component {
                    name,
                    props,
                    children: Some(body),
                    line,
                });
            }
            return Ok(());
        }

        let mut props: Vec<Prop> = Vec::new();
        let self_closing = loop {
            match b.get(j) {
                None => return Err(self.err(start, format!("unclosed <{name}> tag"))),
                Some(b'>') => {
                    j += 1;
                    break false;
                }
                Some(b'/') if b.get(j + 1) == Some(&b'>') => {
                    j += 2;
                    break true;
                }
                // `{row}` is `row={row}`; `{:...props}` gives each key of a
                // browser object as a prop (named `...`).
                Some(b'{') => {
                    let e = hole_end(b, j + 1).ok_or_else(|| self.err(j, "unclosed {".into()))?;
                    let prop = self.src[j + 1..e].trim();
                    if let Some(js) = prop.strip_prefix(':').map(str::trim).and_then(|p| p.strip_prefix("...")) {
                        if js.trim().is_empty() {
                            return Err(self.err(j, "`{:...}` spreads an object: <Card {:...props} />".into()));
                        }
                        let line = self.line_of(j);
                        props.push(Prop { name: "...".into(), value: PropValue::Live(Code { src: js.trim().into(), line }) });
                        j = skip_ws(e + 1);
                        continue;
                    }
                    if prop.starts_with("...") {
                        return Err(self.err(j, format!("a component's props are spread from a browser value: {{:{prop}}}; Rust props are given one by one")));
                    }
                    if !is_ident(prop) {
                        return Err(self.err(j, "in a component's tag, {name} is name={name}: one name in the braces".into()));
                    }
                    if props.iter().any(|p| p.name == prop) {
                        return Err(self.err(j, format!("<{name}> is given `{prop}` twice")));
                    }
                    let line = self.line_of(j);
                    props.push(Prop { name: prop.into(), value: PropValue::Expr(Code { src: prop.into(), line }) });
                    j = e + 1;
                }
                Some(&c) if c.is_ascii_alphabetic() || c == b'_' => {
                    let at = j;
                    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                        j += 1;
                    }
                    let mut prop = self.src[at..j].to_string();
                    // `client:visible`: when the component's browser code starts.
                    if prop == "client" && b.get(j) == Some(&b':') {
                        let n = j + 1;
                        j = n;
                        while j < b.len() && b[j].is_ascii_alphabetic() {
                            j += 1;
                        }
                        let how = &self.src[n..j];
                        let raw = format!("client:{how}");
                        if !matches!(how, "load" | "visible" | "idle" | "interaction" | "media" | "none") {
                            return Err(self.err(at, format!("`{raw}` is not a way to start: client:load (the default), client:visible, client:idle, client:interaction, client:media=\"(query)\" or client:none")));
                        }
                        if props.iter().any(|p| p.name.starts_with("client:")) {
                            return Err(self.err(at, format!("<{name}> starts one way: `{raw}` is a second")));
                        }
                        self.i = j;
                        let value = self.directive_value(&raw)?;
                        j = self.i;
                        let value = match (how, value) {
                            ("media", Some(q)) if !q.src.trim().is_empty() => PropValue::Text(q.src.trim().to_string()),
                            ("media", _) => return Err(self.err(at, "`client:media` needs its query: client:media=\"(min-width: 800px)\"".into())),
                            (_, None) => PropValue::Flag,
                            (_, Some(_)) => return Err(self.err(at, format!("`{raw}` takes no value"))),
                        };
                        props.push(Prop { name: raw, value });
                        j = skip_ws(j);
                        continue;
                    }
                    // `bind:open="x"` and `on:select="pick"`: browser code.
                    if matches!(prop.as_str(), "bind" | "on") && b.get(j) == Some(&b':') {
                        let n = j + 1;
                        j = n;
                        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                            j += 1;
                        }
                        let name_end = j;
                        let raw = self.src[at..j].to_string();
                        if j == n {
                            return Err(self.err(at, format!("`{raw}` needs a name, such as {raw}open")));
                        }
                        self.i = j;
                        let Some(value) = self.directive_value(&raw)?.filter(|v| !v.src.is_empty()) else {
                            return Err(self.err(at, format!("`{raw}` needs a value: {raw}=\"…\", with JavaScript in the quotes")));
                        };
                        j = self.i;
                        let is_bind = prop == "bind";
                        prop = self.src[n..name_end].to_string();
                        if props.iter().any(|p| p.name == prop) {
                            return Err(self.err(at, format!("<{name}> is given `{prop}` twice")));
                        }
                        props.push(Prop { name: prop, value: if is_bind { PropValue::Bind(value) } else { PropValue::On(value) } });
                        j = skip_ws(j);
                        continue;
                    }
                    let k = skip_ws(j);
                    let value = if b.get(k) == Some(&b'=') {
                        let k = skip_ws(k + 1);
                        match b.get(k) {
                            Some(b'{') => {
                                let e = hole_end(b, k + 1).ok_or_else(|| self.err(k, "unclosed {".into()))?;
                                let expr = self.src[k + 1..e].trim();
                                if expr.is_empty() || has_line_comment(expr) {
                                    return Err(self.err(k, format!("`{prop}={{…}}` needs an expression, without // comments")));
                                }
                                j = e + 1;
                                match expr.strip_prefix(':') {
                                    Some(js) if !js.starts_with(':') => PropValue::Live(Code { src: js.trim().to_string(), line: self.line_of(k) }),
                                    _ => PropValue::Expr(Code { src: expr.to_string(), line: self.line_of(k) }),
                                }
                            }
                            Some(&q @ (b'"' | b'\'')) => {
                                let e = k + 1 + b[k + 1..].iter().position(|&c| c == q).ok_or_else(|| self.err(k, "unclosed quote".into()))?;
                                let text = &self.src[k + 1..e];
                                if text.contains('{') {
                                    return Err(self.err(k, format!("a quoted prop is plain text; for an expression write {prop}={{…}}, such as {prop}={{format!(\"…\")}}")));
                                }
                                j = e + 1;
                                PropValue::Text(text.to_string())
                            }
                            _ => return Err(self.err(k, format!("`{prop}=` needs a value: {{expression}} or \"text\""))),
                        }
                    } else {
                        PropValue::Flag
                    };
                    if props.iter().any(|p| p.name == prop) {
                        return Err(self.err(at, format!("<{name}> is given `{prop}` twice")));
                    }
                    props.push(Prop { name: prop, value });
                }
                Some(_) => return Err(self.err(j, format!("<{name}> takes props: name={{expr}}, name=\"text\", or a name alone for true"))),
            }
            j = skip_ws(j);
        };
        self.i = j;
        // Inside a client block, or given browser values, the browser
        // renders it: its props are JavaScript.
        let live = props.iter().any(|p| {
            matches!(
                p.value,
                PropValue::Live(_) | PropValue::Bind(_) | PropValue::On(_)
            )
        });
        if live || self.templates.iter().any(Option::is_some) {
            if let Some(p) = props.iter().find(|p| p.name.starts_with("client:")) {
                return Err(self.err(
                    start,
                    format!("<{name}> is rendered in the browser here, where its code already runs: `{}` is for a component the server renders", p.name),
                ));
            }
            if let Some(p) = props.iter().find(|p| matches!(p.value, PropValue::Expr(_))) {
                return Err(self.err(
                    start,
                    format!("<{name}> is rendered in the browser here, so its props are browser values: write {0}={{:…}} rather than {0}={{…}}", p.name),
                ));
            }
            let d = Directive {
                kind: Dir::Comp,
                name: name.clone(),
                mods: Vec::new(),
                value: None,
                key: None,
                props,
                line: self.line_of(start),
                col: self.col_of(start),
            };
            self.require_text(start, &format!("<{name}>"))?;
            self.begin(start)?;
            let group = self.group(vec![d], self.line_of(start));
            if self_closing {
                self.list().push(Node::Client(vec![(group, Vec::new())]));
                return Ok(());
            }
            self.open(Frame::Client {
                pos: start,
                kind: "comp",
                name,
                conds: Vec::new(),
                has_else: false,
                group,
                branches: Vec::new(),
                body: Vec::new(),
            });
            self.templates.push(Some(Vec::new()));
            return Ok(());
        }
        // A snippet of this file is given as itself, since it is not a value.
        for p in &mut props {
            if let PropValue::Expr(c) = &p.value
                && let Some(s) = self.snippets.iter().rev().find(|s| s.name == c.src)
            {
                p.value = PropValue::Snippet {
                    name: s.name.clone(),
                    arity: s.params.len(),
                };
            }
        }
        if self_closing {
            let line = self.line_of(start);
            self.push_node(
                start,
                Node::Component {
                    name,
                    props,
                    children: None,
                    line,
                },
            )
        } else {
            self.begin(start)?;
            self.open(Frame::Component {
                pos: start,
                name,
                props,
                body: Vec::new(),
            });
            Ok(())
        }
    }

    fn tag_close(&mut self) -> Result<(), Error> {
        if self.tag == "script" && !self.closing && !self.tag_attrs {
            return self.client_script();
        }
        // Any other inline `<script>` is a module: it has a scope of its
        // own, so its top-level names need no `(() => { ... })()` around
        // them, and it runs once the page is parsed, so everything it looks
        // up exists. A script with a `type` or a `src` is left as written.
        if self.tag == "script" && !self.closing && !self.typed {
            self.text.push_str(" type=\"module\"");
        }
        if !self.closing {
            self.form_defaults()?;
            // `<meta http-equiv="refresh" content="0;url=…">` goes to its URL,
            // which no guard checks: its `content` stays static.
            if self.tag == "meta" {
                let refresh = (self.tag_seen.iter()).any(|(a, v)| {
                    a == "http-equiv"
                        && v.as_deref()
                            .is_some_and(|v| v.trim().eq_ignore_ascii_case("refresh"))
                });
                let refreshes = |a: &str| holds_script("meta", a) == Some(Held::Refresh);
                let dynamic = (self.tag_seen.iter()).any(|(a, v)| refreshes(a) && v.is_none())
                    || (self.directives.iter()).any(|d| d.kind == Dir::Attr && refreshes(&d.name));
                if refresh && dynamic {
                    return Err(self.err(self.tag_pos, "no expressions in the `content` of a refresh: it is a URL that may run script; redirect from the server instead".into()));
                }
            }
            if !self.tag_classes.is_empty() {
                // No `class` attribute to put them in: write one.
                let slash = self.last == b'/' && self.text.ends_with('/');
                if slash {
                    self.text.pop();
                    self.text.truncate(self.text.trim_end().len());
                }
                self.text.push_str(" class=\"");
                self.write_classes()?;
                self.text.push('"');
                if slash {
                    self.text.push('/');
                }
            }
            if self.tag == "wisp:element" && !self.directives.iter().any(|d| d.kind == Dir::Tag) {
                return Err(self.err(
                    self.tag_pos,
                    "<wisp:element> needs its tag, from the browser's code: this={:tag}".into(),
                ));
            }
            if self.directives.first().is_some_and(|d| d.kind == Dir::At) {
                let t = self.tag.clone();
                if !(self.last == b'/' && self.text.ends_with('/')) {
                    return Err(self.err(self.tag_pos, format!("<{t}> closes itself: <{t} … />")));
                }
                self.text.pop();
                self.live_element()?;
                self.push_byte(b'>');
                self.text.push_str("</template>");
                self.ctx = Ctx::Text;
                return Ok(());
            }
            self.live_element()?;
        }
        let self_closed = self.last == b'/' && self.text.ends_with('/');
        self.push_byte(b'>');
        self.ctx = Ctx::Text;
        if let Some(name) = self.problem.take() {
            let line = self.line_of(self.tag_pos);
            let auto = true;
            self.push_node(self.tag_pos, Node::Problem { name, line, auto })?;
        }
        if !self.closing && self.tag == "textarea" && self.keep.is_some() {
            // Its content starts in a list of its own.
            self.flush()?;
            let at = self.list().len();
            if let Some(k) = &mut self.keep {
                k.at = at;
            }
        }
        if self.closing {
            match self.tag.as_str() {
                "template" => {
                    self.templates.pop();
                }
                "form" => {
                    self.forms.pop();
                }
                "svg" => self.svg = self.svg.saturating_sub(1),
                "button" if std::mem::take(&mut self.button_form) => self.text.push_str("</form>"),
                "title" if std::mem::take(&mut self.auto_head) => {
                    if !matches!(self.frames.last(), Some(Frame::Head { .. })) {
                        return Err(self.unexpected_close(self.tag_pos, "</title>"));
                    }
                    if let Frame::Head { body, .. } = self.end(self.i)? {
                        self.list().push(Node::Head(body));
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        if self.tag == "svg" && !self_closed {
            self.svg += 1;
        }
        match self.tag.as_str() {
            "pre" | "textarea" => self.preserve += 1,
            "script" | "style" => {
                // Raw text: no holes, no whitespace changes, until the end tag.
                let close = format!("</{}", self.tag);
                let hay = &self.src[self.i..];
                let n = hay
                    .as_bytes()
                    .windows(close.len())
                    .position(|w| w.eq_ignore_ascii_case(close.as_bytes()))
                    .ok_or_else(|| self.err(self.tag_pos, format!("unclosed <{}>", self.tag)))?;
                self.text.push_str(&hay[..n]);
                self.i += n;
            }
            _ => {}
        }
        Ok(())
    }

    /// What a form posting to an action needs and does not say: a
    /// `<form action="?/add">` posts (`method="post"`), and in it (or in a
    /// `<form method="post">`, which posts to `default`) a named `<input>`,
    /// `<textarea>` or `<select>` shows what was sent again when the action
    /// refused it (`Node::Kept`; else its own value, if any), and after it
    /// what was wrong (`Node::Problem`), unless the file shows that field's
    /// problem itself. A password or file is not sent back; its problem is.
    fn form_defaults(&mut self) -> Result<(), Error> {
        let seen = |n: &str| self.tag_seen.iter().find(|(a, _)| a == n);
        let mut method = false;
        if self.tag == "form" {
            let action = self.attr_prefix(self.tag_pos, "action");
            let to_action = action.and_then(|v| v.strip_prefix("?/"));
            let to_default = seen("action").is_none()
                && seen("method")
                    .and_then(|(_, v)| v.as_deref())
                    .is_some_and(|m| m.eq_ignore_ascii_case("post"));
            method = to_action.is_some() && seen("method").is_none();
            let gets = seen("method")
                .and_then(|(_, v)| v.as_deref())
                .is_some_and(|m| !m.eq_ignore_ascii_case("post"));
            let posts = match to_action {
                Some(_) if gets => None,
                Some(v) => Some(self.action_of(v)),
                None => to_default.then(|| "default".to_string()),
            };
            self.forms.push(posts);
        }
        // A button that posts to another action skips this form's browser
        // checks: they are this form's action's, not that one's.
        let novalidate = matches!(self.tag.as_str(), "button" | "input")
            && seen("formnovalidate").is_none()
            && (self.attr_prefix(self.tag_pos, "formaction"))
                .and_then(|v| v.strip_prefix("?/"))
                .is_some_and(|v| {
                    let to = self.action_of(v);
                    (self.forms.last())
                        .is_some_and(|f| f.as_ref().is_some_and(|f| *f != to || to.is_empty()))
                });
        let in_browser = !self.templates.is_empty()
            || !self.rendering.is_empty()
            || self
                .frames
                .iter()
                .any(|f| matches!(f, Frame::Client { .. }));
        let typed = seen("type")
            .and_then(|(_, v)| v.as_deref())
            .unwrap_or("")
            .to_ascii_lowercase();
        // Whether it is sent back, and whether its problem is shown.
        let (keeps, problem) = match self.tag.as_str() {
            "input" => match typed.as_str() {
                "password" | "file" => (false, true),
                "hidden" | "checkbox" | "radio" | "submit" | "button" | "image" | "reset" => {
                    (false, false)
                }
                _ => (true, true),
            },
            "textarea" | "select" => (true, true),
            _ => (false, false),
        };
        let bound = self
            .directives
            .iter()
            .any(|d| d.name == "value" || d.kind == Dir::Spread);
        let name = seen("name")
            .and_then(|(_, v)| v.clone())
            .filter(|_| problem && self.forms.iter().any(Option::is_some) && !in_browser && !bound);
        let chosen = self.tag == "option"
            && !in_browser
            && seen("selected").is_none()
            && self.keep.as_ref().is_some_and(|k| !k.textarea);
        if !method && name.is_none() && !chosen && !novalidate {
            return Ok(());
        }
        // Added before a self-closing tag's `/`.
        let slash = self.last == b'/' && self.text.ends_with('/');
        if slash {
            self.text.pop();
        }
        self.text.truncate(self.text.trim_end().len());
        if method {
            self.text.push_str(" method=\"post\"");
        }
        if novalidate {
            self.text.push_str(" formnovalidate");
        }
        if chosen {
            self.choose_option()?;
        }
        if let Some(name) = name {
            let action = self.forms.iter().rev().find_map(Option::as_deref);
            let field =
                (self.fields.iter()).find(|f| Some(f.action.as_str()) == action && f.name == name);
            if let Some(f) = field {
                let has = |a: &str| {
                    self.tag_seen.iter().any(|(n, _)| n == a)
                        || self.directives.iter().any(|d| d.name == a)
                };
                let attrs = f.native.attrs(&self.tag, &typed, &has);
                self.text.push_str(&attrs);
            }
            if self.tag == "input" {
                if keeps {
                    self.keep_value(&name)?;
                }
                self.problem = Some(name);
            } else {
                if self.tag == "select" {
                    self.choose(&name)?;
                }
                self.keep = Some(Keep {
                    name,
                    textarea: self.tag == "textarea",
                    frames: self.frames.len(),
                    at: 0,
                });
            }
        }
        if slash {
            self.text.push('/');
        }
        Ok(())
    }

    /// A block's body of just `node`, between empty text, as lists go.
    fn alone(&mut self, node: Node) -> Vec<Node> {
        let at = self.chunks.len();
        self.chunks.push(String::new());
        self.chunks.push(String::new());
        vec![Node::Text(at), node, Node::Text(at + 1)]
    }

    /// The index in the current list of the `value={expr}` of the tag being
    /// scanned, if it has one.
    fn value_attr(&mut self) -> Option<usize> {
        if self.frames.len() != self.tag_frames {
            return None;
        }
        let from = self.tag_nodes;
        let list = self.list();
        (from..list.len()).find(|&k| matches!(&list[k], Node::Attr { name, .. } if name == "value"))
    }

    /// The `value` attribute just ended: where, if it is written as text.
    fn value_ends(&mut self) {
        if self.attr == "value"
            && let Some((chunks, _, end)) = &mut self.value_at
            && *chunks == self.chunks.len()
        {
            *end = self.text.len();
        }
    }

    /// The tag's own value, the one rule of every kept field: its
    /// `value={expr}`'s code, else its `value="text"` as a string literal
    /// (entity-decoded, as the browser sends it), else, for an
    /// `<option>`, its text when that is plain. The text is taken out of
    /// the tag when `take` is set: it is the `own` of a `Node::Kept`.
    fn own_value(&mut self, take: bool) -> Option<String> {
        let at = self.value_attr();
        if let Some(Node::Attr { code, .. }) = at.map(|k| &self.list()[k]) {
            return Some(format!("&({})", code.src));
        }
        let lit = |s: &str| format!("{:?}", decode(s));
        match self.tag_seen.iter().find(|(a, _)| a == "value") {
            Some((_, Some(v))) => {
                let v = lit(v);
                // Its text is cut out, from the space before it, when no
                // node has come since: else it is left as written.
                match self.value_at {
                    Some((chunks, at, end)) if chunks == self.chunks.len() && end > at => {
                        if take {
                            let at = self.text[..at].trim_end().len();
                            self.text.replace_range(at..end, "");
                        }
                        Some(v)
                    }
                    _ if !take => Some(v),
                    _ => None,
                }
            }
            Some(_) => None,
            // An option's text, if no `{` or tag comes before its end.
            None if self.tag == "option" => {
                let rest = &self.src[self.i + 1..];
                let text = &rest[..rest.find('<')?];
                let words: Vec<&str> = text.split_ascii_whitespace().collect();
                (!text.contains('{')).then(|| lit(&words.join(" ")))
            }
            None => None,
        }
    }

    /// An `<input name>`'s value: what was sent, when the action refused
    /// it, else its own (`own_value`).
    fn keep_value(&mut self, name: &str) -> Result<(), Error> {
        let line = self.line_of(self.tag_pos);
        let at = self.value_attr();
        // `value="…"` written as text moves into the node; one that cannot
        // is left as it is, never written twice.
        let text = at.is_none() && self.tag_seen.iter().any(|(a, _)| a == "value");
        let lit = text.then(|| self.own_value(true));
        let value = Node::Attr {
            name: "value".into(),
            code: Code {
                src: "__k".into(),
                line,
            },
            url: false,
        };
        let sent = self.alone(value);
        let name = name.to_string();
        match at {
            Some(k) => {
                let own = std::mem::replace(&mut self.list()[k], Node::Render);
                let own = Some(self.alone(own));
                self.list()[k] = Node::Kept {
                    name,
                    sent,
                    own,
                    line,
                };
                Ok(())
            }
            None => {
                let own = match lit {
                    Some(Some(src)) => {
                        let code = Code { src, line };
                        let value = "value".into();
                        Some(self.alone(Node::Attr {
                            name: value,
                            code,
                            url: false,
                        }))
                    }
                    Some(None) => return Ok(()),
                    None => None,
                };
                self.push_node(
                    self.tag_pos,
                    Node::Kept {
                        name,
                        sent,
                        own,
                        line,
                    },
                )
            }
        }
    }

    /// A `<select name>`: the value its options are chosen by, in
    /// `__wisp_sel`: what was sent, when the action refused it, else its
    /// `value={expr}` (which a `<select>` does not otherwise have).
    fn choose(&mut self, name: &str) -> Result<(), Error> {
        let line = self.line_of(self.tag_pos);
        let at = self.value_attr();
        let own = match at.map(|k| &self.list()[k]) {
            Some(Node::Attr { code, .. }) => Some(code.src.clone()),
            _ => None,
        };
        let node = Node::Chosen {
            name: name.to_string(),
            own,
            line,
        };
        match at {
            Some(k) => {
                self.list()[k] = node;
                Ok(())
            }
            None => self.push_node(self.tag_pos, node),
        }
    }

    /// An `<option>` of such a `<select>`: `selected` when its value (see
    /// `own_value`) is the one chosen.
    fn choose_option(&mut self) -> Result<(), Error> {
        let Some(value) = self.own_value(false) else {
            return Ok(());
        };
        let line = self.line_of(self.tag_pos);
        self.push_node(
            self.tag_pos,
            Node::Bool {
                name: "selected".into(),
                code: Code {
                    src: format!("{IS}{value})"),
                    line,
                },
                class: false,
            },
        )
    }

    /// The end tag of a `<textarea>` or `<select>` starts at `pos`: a kept
    /// textarea's content is what was sent, when the action refused it,
    /// else its own; the problem goes after the end tag.
    fn end_keep(&mut self, pos: usize, textarea: bool) -> Result<(), Error> {
        let Some(k) = self.keep.take_if(|k| k.textarea == textarea) else {
            return Ok(());
        };
        if textarea && self.frames.len() == k.frames {
            self.flush()?;
            let line = self.line_of(pos);
            // (A `{:else}` inside it moved on to a list of its own.)
            let list = self.list();
            let own = Some(list.split_off(k.at.min(list.len())));
            let sent = self.alone(Node::Expr(Code {
                src: "__k".into(),
                line,
            }));
            self.list().push(Node::Kept {
                name: k.name.clone(),
                sent,
                own,
                line,
            });
        }
        self.problem = Some(k.name);
        Ok(())
    }

    /// Whether `v`, the end of what `attr_prefix` found, is the end of the
    /// value too: no `{` comes after it.
    fn plain_after(&self, v: &str) -> bool {
        let end = v.as_ptr() as usize - self.src.as_ptr() as usize + v.len();
        self.b.get(end) != Some(&b'{')
    }

    /// The action `?/name&id={x}` (or all of a plain `?/name`) posts to, of
    /// the value past its `?/`: "" when an expression names it.
    fn action_of(&self, v: &str) -> String {
        match v.split_once('&') {
            Some((name, _)) => name.to_string(),
            None if self.plain_after(v) => v.to_string(),
            None => String::new(),
        }
    }

    /// The plain start of attribute `name`'s value (up to its end or its
    /// first `{`) in the tag at `pos` of the source, if the tag has one.
    fn attr_prefix(&self, pos: usize, name: &str) -> Option<&str> {
        let b = self.b;
        let mut i = pos + 1;
        while i < b.len() && !is_ws(b[i]) && !matches!(b[i], b'>' | b'/') {
            i += 1;
        }
        // Past a value (or a hole) whose first byte is at `i`, till `stop`.
        let skip = |mut i: usize, stop: &dyn Fn(u8) -> bool| -> Option<usize> {
            while i < b.len() && !stop(b[i]) {
                i = if b[i] == b'{' {
                    hole_end(b, i + 1)? + 1
                } else {
                    i + 1
                };
            }
            Some(i)
        };
        loop {
            while i < b.len() && (is_ws(b[i]) || b[i] == b'/') {
                i += 1;
            }
            match *b.get(i)? {
                b'>' => return None,
                b'{' => {
                    i = hole_end(b, i + 1)? + 1;
                    continue;
                }
                _ => {}
            }
            let start = i;
            while i < b.len() && !is_ws(b[i]) && !matches!(b[i], b'=' | b'>' | b'{' | b'/') {
                i += 1;
            }
            let attr = &self.src[start..i];
            while i < b.len() && is_ws(b[i]) {
                i += 1;
            }
            if b.get(i) != Some(&b'=') {
                continue;
            }
            i += 1;
            while i < b.len() && is_ws(b[i]) {
                i += 1;
            }
            let quote = matches!(b.get(i), Some(b'"' | b'\'')).then(|| b[i]);
            let from = i + usize::from(quote.is_some());
            let end = |c: u8| match quote {
                Some(q) => c == q,
                None => is_ws(c) || c == b'>',
            };
            if attr.eq_ignore_ascii_case(name) {
                let plain = (from..b.len()).find(|&j| b[j] == b'{' || end(b[j]))?;
                return Some(&self.src[from..plain]);
            }
            i = skip(from, &end)? + usize::from(quote.is_some());
        }
    }

    // ---- browser code -----------------------------------------------------

    /// A `<script>` without attributes is the file's client script. It
    /// leaves the HTML: the build makes it a module, which runs once for
    /// each rendered copy of this file (see `codegen`).
    fn client_script(&mut self) -> Result<(), Error> {
        let at = self.tag_pos;
        if !self.frames.is_empty() || !self.templates.is_empty() {
            return Err(self.err(
                at,
                "a <script> without attributes is this file's client script, which goes at the top level, outside blocks, \
                 <template> and <wisp:head>. To keep a script where it is, give it an attribute such as type=\"module\""
                    .into(),
            ));
        }
        if let Some(first) = &self.script {
            return Err(self.err(
                at,
                format!(
                    "a file has one client script (a <script> without attributes), and this file's is on line {}: \
                     put this code there, or give this script an attribute such as type=\"module\" to keep it as it is",
                    first.line
                ),
            ));
        }
        self.text.truncate(self.tag_text);
        let body = self.i + 1;
        let hay = &self.src[body..];
        let n = hay
            .as_bytes()
            .windows(8)
            .position(|w| w.eq_ignore_ascii_case(b"</script"))
            .ok_or_else(|| self.err(at, "unclosed <script>".into()))?;
        let end = hay[n..].find('>').map_or(hay.len(), |e| n + e + 1);
        self.script = Some(Script {
            src: hay[..n].to_string(),
            line: self.line_of(body),
            col: self.col_of(body),
        });
        self.i = body + end;
        self.ctx = Ctx::Text;
        Ok(())
    }

    /// A directive whose name, at `start`, was just copied into `text`. It
    /// comes back out, with the whitespace before it, and is kept for
    /// `live_element` along with its value.
    fn directive(&mut self, start: usize) -> Result<(), Error> {
        let src = self.src;
        let raw = &src[start..self.i];
        self.text.truncate(self.text.len() - raw.len());
        self.text.truncate(self.text.trim_end().len());
        self.attr.clear();
        if self.frames.len() != self.tag_frames {
            return Err(self.err(start, format!("`{raw}` cannot be inside a {{#…}} block; put the condition in its JavaScript instead")));
        }
        if raw.starts_with("class:") && self.next_is_hole() {
            // Read ahead by `server_classes`; it joins the `class` attribute.
            let open = self.i
                + self.b[self.i..]
                    .iter()
                    .position(|&c| c == b'{')
                    .unwrap_or(0);
            self.i = hole_end(self.b, open + 1).map_or(self.b.len(), |e| e + 1);
            return Ok(());
        }
        let mut value = self.directive_value(raw)?;
        let (kind, mut name, mut mods) =
            directive_parts(raw, &self.tag).map_err(|m| self.err(start, m))?;
        if kind == Dir::Attr && self.held(&name).is_some() {
            return Err(self.err(
                start,
                format!("no `{raw}`: its value can run script; for events use on:click=\"…\""),
            ));
        }
        // `bind:value`, `class:open`, `style:color` alone: the variable of
        // that name.
        if value.is_none()
            && matches!(kind, Dir::Bind | Dir::Class | Dir::Style)
            && is_ident(&name)
            && !js::is_reserved(&name)
        {
            value = Some(Code {
                src: name.clone(),
                line: self.line_of(start),
            });
        }
        if kind == Dir::Wait {
            let how = name.clone();
            match (how.as_str(), value) {
                ("", None) => return Ok(()), // client:load, as without it
                (ISLAND_MEDIA, Some(q)) if !q.src.is_empty() => {
                    name = format!("{ISLAND_MEDIA}{}", q.src);
                }
                (ISLAND_MEDIA, _) => {
                    return Err(self.err(
                        start,
                        "`client:media` needs its query: client:media=\"(min-width: 800px)\""
                            .into(),
                    ));
                }
                (_, Some(_)) => return Err(self.err(start, format!("`{raw}` takes no value"))),
                _ => {}
            }
            value = None;
        }
        let needs_value = !matches!(kind, Dir::Transition | Dir::Use | Dir::Animate | Dir::Wait);
        if needs_value && value.as_ref().is_none_or(|v| v.src.is_empty()) {
            return Err(self.err(
                start,
                format!("`{raw}` needs a value: {raw}=\"…\", with JavaScript in the quotes"),
            ));
        }
        let (name, value) = match (kind, value) {
            (Dir::Each, Some(v)) => {
                let Some((item, index, at)) = js::each(&v.src) else {
                    return Err(self.err(start, "expected <template each=\"item in list\"> or <template each=\"item, i in list\">".into()));
                };
                mods.extend(index);
                let line = v.line + v.src[..at].matches('\n').count() as u32;
                (
                    item,
                    Some(Code {
                        src: v.src[at..].trim_end().to_string(),
                        line,
                    }),
                )
            }
            (_, value) => (name, value),
        };
        let (line, col) = (self.line_of(start), self.col_of(start));
        self.directives.push(Directive {
            kind,
            name,
            mods,
            value,
            key: None,
            props: Vec::new(),
            line,
            col,
        });
        Ok(())
    }

    /// `="…"` or `='…'` after a directive's name, if there is one. It is
    /// JavaScript, taken as written: no holes, no whitespace changes.
    fn directive_value(&mut self, raw: &str) -> Result<Option<Code>, Error> {
        let b = self.b;
        let skip_ws = |mut j: usize| {
            while j < b.len() && is_ws(b[j]) {
                j += 1;
            }
            j
        };
        let eq = skip_ws(self.i);
        if b.get(eq) != Some(&b'=') {
            return Ok(None);
        }
        let j = skip_ws(eq + 1);
        match b.get(j) {
            Some(&q @ (b'"' | b'\'')) => {
                let e = j + 1 + b[j + 1..].iter().position(|&c| c == q).ok_or_else(|| self.err(j, "unclosed quote".into()))?;
                self.i = e + 1;
                let value = &self.src[j + 1..e];
                let lead = value.len() - value.trim_start().len();
                Ok(Some(Code { src: value.trim().to_string(), line: self.line_of(j + 1 + lead) }))
            }
            Some(b'{') => Err(self.err(j, format!("`{raw}` takes JavaScript in quotes, {raw}=\"…\": braces are Rust, which runs on the server"))),
            _ => Err(self.err(j, format!("`{raw}=` needs its JavaScript in quotes: {raw}=\"…\""))),
        }
    }

    /// `={` follows: the value is Rust, not JavaScript in quotes.
    fn next_is_hole(&self) -> bool {
        let rest = self.src[self.i..].trim_start();
        rest.strip_prefix('=')
            .is_some_and(|r| r.trim_start().starts_with('{'))
    }

    /// The `class:name={cond}` attributes of the tag whose attributes start
    /// at `i`. They are found before the tag is read because they go into its
    /// `class` attribute, which may come first.
    fn server_classes(&self, mut i: usize) -> Vec<(String, Code)> {
        let b = self.b;
        let mut found = Vec::new();
        while i < b.len() && b[i] != b'>' {
            match b[i] {
                b'{' => i = hole_end(b, i + 1).unwrap_or(b.len()),
                q @ (b'"' | b'\'') => {
                    i += 1;
                    while i < b.len() && b[i] != q {
                        if b[i] == b'{' {
                            i = hole_end(b, i + 1).unwrap_or(b.len());
                        }
                        i += 1;
                    }
                }
                b'c' if is_ws(b[i - 1]) && self.src[i..].starts_with("class:") => {
                    let name = &self.src[i + 6..];
                    let n = name
                        .find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '>' | '/'))
                        .unwrap_or(name.len());
                    let at = i + 6 + n;
                    if let Some(open) = b[at..].starts_with(b"={").then_some(at + 1)
                        && let Some(e) = hole_end(b, open + 1)
                    {
                        let src = self.src[open + 1..e].trim().to_string();
                        let line = self.line_of(open);
                        found.push((name[..n].to_string(), Code { src, line }));
                        i = e;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        found
    }

    /// Where a tag's `class` value ends: its `class:name={cond}` add their
    /// names, ` name` for each that holds.
    fn write_classes(&mut self) -> Result<(), Error> {
        for (name, code) in std::mem::take(&mut self.tag_classes) {
            self.push_node(
                self.i,
                Node::Bool {
                    name,
                    code,
                    class: true,
                },
            )?;
        }
        Ok(())
    }

    /// The tag being closed, if it has directives, gets a `Live` node just
    /// before its `>` (or `/>`). A `<template>` also starts or ends the
    /// names its `each` gives the elements inside it.
    fn live_element(&mut self) -> Result<(), Error> {
        let mut directives = std::mem::take(&mut self.directives);
        // Where the rest go, when they start, and the tag come first: the
        // runtime reads them before the others.
        directives.sort_by_key(|d| !matches!(d.kind, Dir::At | Dir::Wait | Dir::Tag));
        let client = directives
            .iter()
            .find(|d| matches!(d.kind, Dir::Each | Dir::If));
        if client.is_some() && directives.len() > 1 {
            return Err(self.err(self.tag_pos, "a <template> with `each` or `if` takes no other directive; put them on the elements inside it".into()));
        }
        let names = client.map(|d| {
            if d.kind == Dir::Each {
                std::iter::once(d.name.clone())
                    .chain(d.mods.iter().cloned())
                    .collect()
            } else {
                Vec::new()
            }
        });
        if !directives.is_empty() {
            // `<input … />` keeps its `/` last.
            let slash = self.last == b'/' && self.text.ends_with('/');
            if slash {
                self.text.pop();
                self.text.truncate(self.text.trim_end().len());
            }
            let (nested, locals) = (
                self.templates.iter().any(Option::is_some),
                self.templates.iter().flatten().flatten().cloned().collect(),
            );
            let g = Group {
                directives,
                locals,
                nested,
                line: self.line_of(self.tag_pos),
            };
            // A `<wisp:element>`'s group was taken with its name.
            let group = match self.elements.last() {
                Some(&e) if self.tag == "wisp:element" => {
                    self.groups[e] = g;
                    e
                }
                _ => {
                    self.groups.push(g);
                    self.groups.len() - 1
                }
            };
            self.push_node(self.i, Node::Live { group })?;
            if slash {
                self.text.push('/');
            }
        }
        if self.tag == "template" {
            self.templates.push(names);
        }
        Ok(())
    }

    /// A group for `directives` on the element being written.
    fn group(&mut self, directives: Vec<Directive>, line: u32) -> usize {
        let (nested, locals) = (
            self.templates.iter().any(Option::is_some),
            self.templates.iter().flatten().flatten().cloned().collect(),
        );
        self.groups.push(Group {
            directives,
            locals,
            nested,
            line,
        });
        self.groups.len() - 1
    }

    /// Client blocks, holes and components go where text does.
    fn require_text(&self, pos: usize, what: &str) -> Result<(), Error> {
        if self.ctx != Ctx::Text {
            return Err(self.err(
                pos,
                format!("{what} goes in text, not {}", self.place().describe()),
            ));
        }
        Ok(())
    }

    /// `<template>` with a `protocol::GROUP_ATTR`, the anchor of a client block, hole
    /// or component, whose one directive is `d`.
    fn client_template(&mut self, d: Directive) -> Result<(), Error> {
        let line = d.line;
        self.text.push_str("<template");
        let group = self.group(vec![d], line);
        self.push_node(self.i, Node::Live { group })?;
        self.text.push('>');
        Ok(())
    }

    /// `{:#if cond}` or `{:#each list as item, i (key)}` at `open`.
    fn client_open(&mut self, open: usize, kw: &str, arg: &str) -> Result<(), Error> {
        self.require_text(open, &format!("{{:#{kw}}}"))?;
        let line = self.line_of(open);
        let js = |s: &str| Code {
            src: s.trim().to_string(),
            line,
        };
        let mut d = Directive {
            kind: Dir::If,
            name: String::new(),
            mods: Vec::new(),
            value: Some(js(arg)),
            key: None,
            props: Vec::new(),
            line,
            col: self.col_of(open),
        };
        let (kind, conds, names) = match kw {
            "if" => ("if", vec![arg.trim().to_string()], Vec::new()),
            "each" => {
                let bad = || {
                    self.err(open, "expected {:#each list as item}, {:#each list as item, i} or with a key: {:#each list as item, i (item.id)}".into())
                };
                // `{:#each list}` alone draws its content once per item.
                let (list, pat, key) = match split_client_each(arg) {
                    Some(x) => x,
                    None if !arg.contains(" as ") && !arg.trim_end().ends_with(" as") => {
                        (arg, "", None)
                    }
                    None => return Err(bad()),
                };
                let (item, index) = match pat.split_once(',') {
                    Some((a, b)) => (a.trim(), Some(b.trim())),
                    None => (pat.trim(), None),
                };
                let ident = |s: &str| {
                    s.bytes()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_' || c == b'$')
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$')
                        && !js::is_reserved(s)
                };
                if !(ident(item) || pat.is_empty()) || index.is_some_and(|i| !ident(i)) {
                    return Err(bad());
                }
                d.kind = Dir::Each;
                d.name = item.to_string();
                d.mods.extend(index.map(String::from));
                d.value = Some(js(list));
                d.key = key.map(js);
                let names: Vec<String> = std::iter::once(item.to_string())
                    .chain(index.map(String::from))
                    .collect();
                ("each", vec![list.trim().to_string()], names)
            }
            // `{:#key x}`: drawn afresh when x changes.
            "key" => {
                d.kind = Dir::Key;
                ("key", Vec::new(), Vec::new())
            }
            // `{:#await p}…{:then v}…{:catch e}…{:/await}` and `{:#try}…
            // {:catch e}…{:/try}`: one copy with a local saying how it went,
            // around a block per branch that reads it (see `client_branch`).
            "await" | "try" => {
                let (promise, then) = match arg.find(" then") {
                    Some(n)
                        if kw == "await"
                            && arg[n + 5..]
                                .trim()
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$') =>
                    {
                        (&arg[..n], Some(arg[n + 5..].trim()))
                    }
                    _ => (arg, None),
                };
                if kw == "try" && !arg.trim().is_empty() {
                    return Err(self.err(
                        open,
                        "{:#try} takes nothing: {:#try}…{:catch error}…{:/try}".into(),
                    ));
                }
                d.kind = if kw == "await" { Dir::Await } else { Dir::Try };
                d.value = (kw == "await").then(|| js(promise));
                let local = if kw == "await" { "__aw" } else { "__tr" };
                let outer = if kw == "await" { "await" } else { "try" };
                self.client_frame(open, outer, Vec::new(), d, vec![local.into()])?;
                let first = match then {
                    Some(v) => ("then", v),
                    None if kw == "await" => ("pending", ""),
                    None => ("body", ""),
                };
                self.client_branch(open, first.0, first.1)?;
                self.skip_standalone(open);
                return Ok(());
            }
            _ => {
                return Err(self.err(
                    open,
                    format!(
                        "unknown block {{:#{kw}}}: the browser's blocks are {{:#if}}, {{:#each}}, {{:#key}}, {{:#await}} and {{:#try}}"
                    ),
                ));
            }
        };
        self.client_frame(open, kind, conds, d, names)?;
        self.skip_standalone(open);
        Ok(())
    }

    /// Opens a client block of `kind` at `open`, whose `<template>` has the
    /// one directive `d` and gives the elements inside it `names`.
    fn client_frame(
        &mut self,
        open: usize,
        kind: &'static str,
        conds: Vec<String>,
        d: Directive,
        names: Vec<String>,
    ) -> Result<(), Error> {
        self.begin(open)?;
        let line = d.line;
        let group = self.group(vec![d], line);
        self.open(Frame::Client {
            pos: open,
            kind,
            name: String::new(),
            conds,
            has_else: false,
            group,
            branches: Vec::new(),
            body: Vec::new(),
        });
        self.templates.push(Some(names));
        Ok(())
    }

    /// A branch of `{:#await}` or `{:#try}`: `pending`, `then` or `catch`
    /// (an await's), `body` or `failed` (a try's), with the name its value
    /// is read as. It is a block of its own, inside the one copy, drawn
    /// when the copy's `__aw` or `__tr` says so.
    fn client_branch(&mut self, open: usize, kind: &'static str, name: &str) -> Result<(), Error> {
        if !name.is_empty() && (!is_ident(name) || js::is_reserved(name)) {
            return Err(self.err(
                open,
                format!("`{name}` is not a name for what the block gives"),
            ));
        }
        let (dir, test) = match kind {
            "pending" => (Dir::If, "!__aw.k"),
            "then" => (Dir::Each, "__aw.k == 1 ? [__aw.x] : []"),
            "catch" => (Dir::Each, "__aw.k == 2 ? [__aw.x] : []"),
            "body" => (Dir::If, "!__tr.f"),
            _ => (Dir::Each, "__tr.f ? [__tr.e] : []"),
        };
        let line = self.line_of(open);
        let d = Directive {
            kind: dir,
            name: name.into(),
            mods: Vec::new(),
            value: Some(Code {
                src: test.into(),
                line,
            }),
            key: None,
            props: Vec::new(),
            line,
            col: self.col_of(open),
        };
        let mut names = if dir == Dir::Each {
            vec![name.to_string()]
        } else {
            Vec::new()
        };
        // A try's `{:catch}` may call `reset()` to draw its body again.
        if kind == "failed" {
            names.push("reset".into());
        }
        self.client_frame(open, kind, Vec::new(), d, names)
    }

    /// `{:else}` or `{:else if cond}` in a client block: the branch before
    /// ends, and a `<template if>` for this one begins, whose condition is
    /// that no branch before it holds.
    fn client_else(&mut self, open: usize, arg: &str) -> Result<(), Error> {
        let line = self.line_of(open);
        let col = self.col_of(open);
        let Some(Frame::Client {
            kind,
            conds,
            has_else,
            ..
        }) = self.frames.last_mut()
        else {
            unreachable!("caller checked")
        };
        let (kind, cond) = (*kind, arg.trim());
        let not = |c: &String| format!("!({c})");
        let test = match (kind, split_word(cond)) {
            _ if *has_else => None,
            ("if", ("", _)) => {
                *has_else = true;
                Some(conds.iter().map(not).collect::<Vec<_>>().join(" && "))
            }
            ("if", ("if", c)) if !c.is_empty() => {
                let t = conds
                    .iter()
                    .map(not)
                    .chain(std::iter::once(format!("({c})")))
                    .collect::<Vec<_>>()
                    .join(" && ");
                conds.push(c.to_string());
                Some(t)
            }
            ("each", ("", _)) => {
                *has_else = true;
                Some(format!("![...({} ?? [])].length", conds[0]))
            }
            _ => None,
        };
        let Some(test) = test else {
            return Err(self.err(
                open,
                format!(
                    "{{:else}} is not allowed here: a {{:#{kind}}} takes {}",
                    if kind == "if" {
                        "{:else if …} and then one {:else}"
                    } else {
                        "one {:else}, shown when the list is empty"
                    }
                ),
            ));
        };
        self.templates.pop();
        let d = Directive {
            kind: Dir::If,
            name: String::new(),
            mods: Vec::new(),
            value: Some(Code { src: test, line }),
            key: None,
            props: Vec::new(),
            line,
            col,
        };
        let next = self.group(vec![d], line);
        if let Some(Frame::Client {
            group,
            branches,
            body,
            ..
        }) = self.frames.last_mut()
        {
            branches.push((*group, std::mem::take(body)));
            *group = next;
        }
        self.templates.push(Some(Vec::new()));
        self.skip_standalone(open);
        Ok(())
    }

    /// The end of the innermost client block or component, at `pos`.
    fn client_close(&mut self, pos: usize) -> Result<(), Error> {
        self.templates.pop();
        if let Frame::Client {
            group,
            mut branches,
            body,
            ..
        } = self.end(pos)?
        {
            branches.push((group, body));
            self.list().push(Node::Client(branches));
        }
        Ok(())
    }

    // ---- snippets ---------------------------------------------------------

    /// `{#snippet name(params)}` at `open`.
    fn snippet_open(&mut self, open: usize, arg: &str) -> Result<(), Error> {
        self.require_text(open, "{#snippet}")?;
        if self.templates.iter().any(Option::is_some) {
            return Err(self.err(
                open,
                "a {#snippet} is defined outside client blocks and browser-drawn components; \
                 draw it in one with {:@render name(…)}"
                    .into(),
            ));
        }
        let usage = "{#snippet name(param, …)}";
        let (name, params) = self.call(open, arg, usage)?;
        if name == "children" {
            return Err(self.err(
                open,
                "`children` is what a component wraps; call the snippet something else".into(),
            ));
        }
        let params: Vec<String> = split_top(&params, b',')
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        self.begin(open)?;
        self.open(Frame::Snippet {
            pos: open,
            name,
            params,
            start: 0,
            body: Vec::new(),
        });
        self.skip_standalone(open);
        if let Some(Frame::Snippet { start, .. }) = self.frames.last_mut() {
            *start = self.i;
        }
        Ok(())
    }

    /// `name(args)` → the name and the arguments. A snippet cannot render
    /// itself, since it would never end; a component can.
    fn call(&self, open: usize, arg: &str, usage: &str) -> Result<(String, String), Error> {
        let (name, args) = arg
            .split_once('(')
            .and_then(|(n, a)| Some((n.trim(), a.strip_suffix(')')?)))
            .filter(|(n, _)| is_ident(n))
            .ok_or_else(|| self.err(open, format!("expected {usage}")))?;
        let open_snippet = self
            .frames
            .iter()
            .any(|f| matches!(f, Frame::Snippet { name: n, .. } if n == name));
        if open_snippet || self.rendering.iter().any(|n| n == name) {
            return Err(self.err(
                open,
                format!("snippet `{name}` renders itself, which never ends; a component can render itself inside a block that ends"),
            ));
        }
        Ok((name.to_string(), args.trim().to_string()))
    }

    fn check_args(&self, open: usize, name: &str, n: usize, args: &str) -> Result<(), Error> {
        let given = split_top(args, b',')
            .iter()
            .filter(|a| !a.trim().is_empty())
            .count();
        if given != n {
            return Err(self.err(
                open,
                format!("snippet `{name}` takes {n} argument(s), and is given {given}"),
            ));
        }
        Ok(())
    }

    /// `{:@render name(args)}`: the browser draws the snippet. Its body is
    /// read again here, inside a one-item `{:#each}` per parameter, so the
    /// arguments are JavaScript and the body sees each by its name.
    fn client_render(&mut self, open: usize, arg: &str) -> Result<(), Error> {
        self.require_text(open, "{:@render}")?;
        let (name, args) = self.call(open, arg, "{:@render name(arg, …)}")?;
        let Some(s) = self.snippets.iter().rev().find(|s| s.name == name) else {
            return Err(self.err(
                open,
                format!("no snippet `{name}` above: {{:@render}} draws a {{#snippet}} defined earlier in this file"),
            ));
        };
        let (params, body) = (s.params.clone(), s.body.clone());
        self.check_args(open, &name, params.len(), &args)?;
        for (p, a) in params.iter().zip(split_top(&args, b',')) {
            let p = untyped(p);
            if !is_ident(p) || js::is_reserved(p) {
                return Err(self.err(
                    open,
                    format!("the browser draws snippet `{name}` here, so its parameters are plain names, not `{p}`"),
                ));
            }
            self.client_open(open, "each", &format!("[{}] as {p}", a.trim()))?;
        }
        let (i, end) = (self.i, self.end);
        (self.i, self.end) = (body.start, body.end);
        self.rendering.push(name);
        self.scan()?;
        self.rendering.pop();
        (self.i, self.end) = (i, end);
        for _ in &params {
            self.client_close(open)?;
        }
        Ok(())
    }

    /// `{:expr}` in text: an anchor the browser puts the value after, what
    /// the server knows of it, and the end of it.
    fn live_text(&mut self, open: usize, js: &str) -> Result<(), Error> {
        let (line, col) = (self.line_of(open), self.col_of(open));
        let d = Directive {
            kind: Dir::Hole,
            name: String::new(),
            mods: Vec::new(),
            value: Some(Code {
                src: js.to_string(),
                line,
            }),
            key: None,
            props: Vec::new(),
            line,
            col,
        };
        self.begin(open)?;
        self.client_template(d)?;
        let group = self.groups.len() - 1;
        self.text.push_str("</template>");
        self.push_node(open, Node::Hole { group })?;
        self.text.push_str("<!---->");
        Ok(())
    }

    /// `{:…}` in a quoted attribute value that began at `value_start`: the
    /// whole value, read here, becomes a live attribute (a template literal),
    /// and the server writes its static text. The closing quote is left for
    /// the main loop.
    fn live_value(&mut self, open: usize, q: u8) -> Result<(), Error> {
        if self.value_events {
            return Err(self.err(open, format!("`{}` mixes {{…}} (the server's) and {{:…}} (the browser's); use one kind in a value", self.attr)));
        }
        self.check_live_attr(open)?;
        // A static start that runs script stays refused, as for `{…}`; the
        // browser guards what the value decides.
        if is_url_attr(&self.attr) {
            match scheme(&self.text[self.value_start..]) {
                Scheme::Script => return Err(self.err(open, format!("no expressions in a `{}` that runs script", self.attr))),
                Scheme::Encoded => return Err(self.err(open, format!("`{}` has a character reference (`&...;`) before its scheme and an expression; write the URL's start plainly", self.attr))),
                Scheme::Fixed | Scheme::Open => {}
            }
        }
        let mut js = String::from("`");
        let lit = |s: &str, js: &mut String| {
            for c in s.chars() {
                if matches!(c, '`' | '\\' | '$') {
                    js.push('\\');
                }
                js.push(c);
            }
        };
        lit(&self.text[self.value_start..], &mut js);
        let mut i = open;
        loop {
            match self.b.get(i) {
                None => return Err(self.err(open, "unclosed quote".into())),
                Some(&c) if c == q => break,
                Some(b'{') => {
                    let e =
                        hole_end(self.b, i + 1).ok_or_else(|| self.err(i, "unclosed {".into()))?;
                    let Some(expr) = self.src[i + 1..e]
                        .trim()
                        .strip_prefix(':')
                        .filter(|x| !x.trim().is_empty())
                    else {
                        return Err(self.err(i, format!("`{}` mixes {{…}} (the server's) and {{:…}} (the browser's); use one kind in a value", self.attr)));
                    };
                    js.push_str("${(");
                    js.push_str(expr.trim());
                    js.push_str(") ?? ''}");
                    i = e + 1;
                }
                Some(_) => {
                    let s = i;
                    while i < self.b.len() && self.b[i] != q && self.b[i] != b'{' {
                        i += 1;
                    }
                    lit(&self.src[s..i], &mut js);
                    self.text.push_str(&self.src[s..i]);
                }
            }
        }
        js.push('`');
        self.i = i;
        let (line, col) = (self.line_of(open), self.col_of(open));
        let name = self.attr.clone();
        self.directives.push(Directive {
            kind: Dir::Attr,
            name,
            mods: Vec::new(),
            value: Some(Code { src: js, line }),
            key: None,
            props: Vec::new(),
            line,
            col,
        });
        Ok(())
    }

    /// The tag as `contexts::holds_script` takes it: "" (any) for a
    /// `<wisp:element>`, whose tag the browser's code chooses.
    fn spread_tag(&self) -> &str {
        if self.tag == "wisp:element" {
            ""
        } else {
            &self.tag
        }
    }

    /// What attribute `name` here holds that escaping does not make safe.
    /// A `<meta>`'s `content` is refused only in a refresh, which
    /// `tag_close` sees whole.
    fn held(&self, name: &str) -> Option<Held> {
        holds_script(self.spread_tag(), name).filter(|&h| h != Held::Refresh)
    }

    /// A live value may go on this attribute here.
    fn check_live_attr(&self, open: usize) -> Result<(), Error> {
        if self.frames.len() != self.tag_frames {
            return Err(self.err(open, "a {:…} value cannot be inside a {#…} block; put the condition in its JavaScript instead".into()));
        }
        if self.attr.is_empty() || self.held(&self.attr).is_some() {
            return Err(self.err(
                open,
                format!(
                    "no {{:…}} in `{}`; for events use on:click=\"…\"",
                    self.attr
                ),
            ));
        }
        Ok(())
    }

    // ---- holes ------------------------------------------------------------

    fn hole(&mut self) -> Result<(), Error> {
        let open = self.i;
        let end = hole_end(self.b, open + 1).ok_or_else(|| {
            self.err(
                open,
                "unclosed {; to show a { as text, write {\"{\"}".into(),
            )
        })?;
        self.i = end + 1;
        let t = self.src[open + 1..end].trim();
        if has_line_comment(t) {
            return Err(self.err(open, "no // comments inside {…}: the code after it would be commented out too. Use /* … */".into()));
        }
        let line = self.line_of(open);
        let code = |s: &str| Code {
            src: s.trim().to_string(),
            line,
        };
        if self.ctx != Ctx::Text {
            self.tag_attrs = true;
        }

        // Blocks may appear anywhere, even inside tags (conditional attributes).
        if let Some(rest) = t.strip_prefix('#') {
            return self.block_open(open, rest);
        }
        if let Some(rest) = t.strip_prefix(':').filter(|_| !t.starts_with("::")) {
            // The browser's: `{:#if}`, `{:/if}`, and `{:expr}`. `else` and
            // `case` are JavaScript keywords, so `{:else}` and `{:case}` are
            // always branches.
            if let Some(call) = rest.strip_prefix("@render") {
                return self.client_render(open, call.trim());
            }
            if let Some(block) = rest.strip_prefix('#') {
                let (kw, arg) = split_word(block);
                if arg.is_empty() && kw != "try" {
                    return Err(self.err(open, format!("{{:#{kw}}} needs an expression")));
                }
                return self.client_open(open, kw, arg);
            }
            if let Some(kw) = rest.strip_prefix('/') {
                let kw = kw.trim();
                let top = match self.frames.last() {
                    Some(Frame::Client { kind, .. }) => block_of(kind),
                    _ => "",
                };
                if top.is_empty() || top != kw || kw == "comp" {
                    return Err(self.unexpected_close(open, &format!("{{:/{kw}}}")));
                }
                // An await's or try's branch, then the block around it.
                if matches!(kw, "await" | "try") {
                    self.client_close(open)?;
                }
                self.client_close(open)?;
                self.skip_standalone(open);
                return Ok(());
            }
            let (kw, arg) = split_word(rest);
            // `{:then v}` and `{:catch e}`: the next branch of an await or try.
            let branch = match self.frames.last() {
                Some(Frame::Client { kind, .. }) => *kind,
                _ => "",
            };
            let next = match (kw, branch) {
                ("then", "pending") => Some("then"),
                ("catch", "pending" | "then") => Some("catch"),
                ("catch", "body") => Some("failed"),
                _ => None,
            };
            if let Some(next) = next {
                self.client_close(open)?;
                self.client_branch(open, next, arg)?;
                self.skip_standalone(open);
                return Ok(());
            }
            if !matches!(kw, "else" | "case" | "elseif" | "elif" | "elsif") {
                let js = rest.trim();
                if js.is_empty() {
                    return Err(self.err(
                        open,
                        "empty {:}: write the JavaScript to show, such as {:count}".into(),
                    ));
                }
                return match self.ctx {
                    Ctx::Text => self.live_text(open, js),
                    Ctx::Quoted(q) => self.live_value(open, q),
                    Ctx::Tag if self.last == b'=' => {
                        self.check_live_attr(open)?;
                        self.unwrite_attr_name(open)?;
                        let (line, col) = (self.line_of(open), self.col_of(open));
                        let name = self.attr.clone();
                        // A whole value (`{}`): the server may paint it.
                        let tag = self.tag == "wisp:element" && name == "this";
                        self.directives.push(Directive {
                            kind: if tag { Dir::Tag } else { Dir::Attr },
                            name,
                            mods: if tag { Vec::new() } else { vec!["{}".into()] },
                            value: Some(Code {
                                src: js.to_string(),
                                line,
                            }),
                            key: None,
                            props: Vec::new(),
                            line,
                            col,
                        });
                        self.last = b'a';
                        Ok(())
                    }
                    Ctx::Tag
                        if js.starts_with("...")
                            && !js[3..].trim().is_empty()
                            && self.frames.len() == self.tag_frames =>
                    {
                        let (line, col) = (self.line_of(open), self.col_of(open));
                        self.directives.push(Directive {
                            kind: Dir::Spread,
                            // Its tag, for the keys it leaves out: none
                            // (any) when the browser's code chooses it.
                            name: self.spread_tag().into(),
                            mods: Vec::new(),
                            value: Some(Code {
                                src: js[3..].trim().to_string(),
                                line,
                            }),
                            key: None,
                            props: Vec::new(),
                            line,
                            col,
                        });
                        Ok(())
                    }
                    Ctx::Tag => Err(self.err(
                        open,
                        "inside a tag, {:…} must be an attribute's value, name={:expr}, or {:...attributes}".into(),
                    )),
                };
            }
            if kw == "else"
                && matches!(
                    self.frames.last(),
                    Some(Frame::Client {
                        kind: "if" | "each",
                        ..
                    })
                )
            {
                self.flush()?;
                return self.client_else(open, arg);
            }
            self.flush()?;
            self.same_place(open, &format!("{{:{kw}}}"))?;
            match (kw, self.frames.last_mut()) {
                (
                    "else",
                    Some(Frame::If {
                        branches,
                        otherwise,
                        ..
                    }),
                ) if otherwise.is_none() => match split_word(arg) {
                    ("", _) => *otherwise = Some(Vec::new()),
                    ("if", c) if !c.is_empty() => branches.push((code(c), Vec::new())),
                    _ => return Err(self.err(open, "expected {:else} or {:else if <cond>}".into())),
                },
                ("else", Some(Frame::Each { otherwise, .. }))
                    if otherwise.is_none() && arg.is_empty() =>
                {
                    *otherwise = Some(Vec::new());
                }
                ("case", Some(Frame::Match { arms, .. })) if !arg.is_empty() => {
                    arms.push((code(arg), Vec::new()))
                }
                ("elseif" | "elif" | "elsif", _) => {
                    return Err(self.err(
                        open,
                        format!("{{:{kw}}} is written {{:else if <condition>}}"),
                    ));
                }
                _ => return Err(self.err(open, format!("{{:{kw}}} is not allowed here"))),
            }
            self.restart();
            self.skip_standalone(open);
            return Ok(());
        }
        if let Some(rest) = t.strip_prefix('/') {
            return self.block_close(open, rest.trim());
        }

        // Output holes: where they may appear depends on the HTML context.
        if self.ctx == Ctx::Tag && self.last != b'=' {
            // `{href}` is `href={href}`.
            if !t.bytes().all(is_word) || t.is_empty() {
                return Err(self.err(
                    open,
                    "inside a tag, expressions must be attribute values: name={expr}, or {name} for name={name}".into(),
                ));
            }
            // Lowercase, as the browser reads it: `{ONCLICK}` is `onclick`.
            self.attr = t.to_ascii_lowercase();
            self.typed |= matches!(self.attr.as_str(), "type" | "src" | "nomodule");
            self.tag_seen.push((self.attr.clone(), None));
            self.text.push_str(t);
            self.text.push('=');
            self.last = b'=';
        }
        let held = if self.ctx == Ctx::Text {
            None
        } else {
            self.held(&self.attr)
        };
        match held {
            Some(Held::Event) => {
                return Err(self.err(
                    open,
                    format!(
                        "no expressions in event handler attributes like `{}`; use data-* attributes",
                        self.attr
                    ),
                ));
            }
            Some(Held::Document) => {
                return Err(self.err(open, "no expressions in `srcdoc`: its value is a whole HTML document, where escaping for an attribute is not enough".into()));
            }
            Some(_) => {
                return Err(self.err(open, format!("no expressions in the `{}` of a <{}>: it can hold a URL that runs script, which escaping does not stop", self.attr, self.tag)));
            }
            None => {}
        }
        let unquoted = self.ctx == Ctx::Tag;
        if unquoted {
            self.last = b'a';
        }
        if self.ctx != Ctx::Text && BOOLEAN_ATTRS.contains(&self.attr.as_str()) {
            // On or off. A value, even "false", would turn it on.
            if !unquoted || t.is_empty() || t.starts_with('@') {
                return Err(self.err(
                    open,
                    format!("`{0}` is on or off: write {0}={{condition}}", self.attr),
                ));
            }
            self.unwrite_attr_name(open)?;
            return self.push_node(
                open,
                Node::Bool {
                    name: self.attr.clone(),
                    code: code(t),
                    class: false,
                },
            );
        }

        if let Some(rest) = t.strip_prefix('@') {
            return self.at_tag(open, rest, unquoted);
        }
        if t.is_empty() {
            return Err(self.err(open, "empty {}".into()));
        }
        // A whole attribute value: `None` leaves the attribute out.
        // (`name = {x}` with spaces cannot be taken back, and is written as text.)
        if unquoted
            && (self.attr != "class" || self.tag_classes.is_empty())
            && self.unwrite_attr_name(open).is_ok()
        {
            let name = self.attr.clone();
            let url = is_url_attr(&name);
            return self.push_node(
                open,
                Node::Attr {
                    name,
                    code: code(t),
                    url,
                },
            );
        }
        if unquoted {
            // `name={expr}`: the value is quoted here, so it cannot end early.
            self.text.push('"');
            self.value_start = self.text.len();
            self.value_events = false;
        }
        if self.ctx != Ctx::Text {
            self.in_value(open)?;
        }
        // `{cx.problem("x")}` in text: the element an input's problem gets.
        if let Some(name) = problem_call(t).filter(|_| self.ctx == Ctx::Text) {
            let line = self.line_of(open);
            self.shown.push(name.clone());
            let auto = false;
            return self.push_node(open, Node::Problem { name, line, auto });
        }
        self.push_node(open, Node::Expr(code(t)))?;
        if unquoted {
            self.end_value(open)?;
            self.write_classes()?;
            self.text.push('"');
        }
        Ok(())
    }

    /// Rust from the hole at `open`, on its line.
    fn code_at(&self, open: usize, s: &str) -> Code {
        Code {
            src: s.trim().to_string(),
            line: self.line_of(open),
        }
    }

    /// `{#if …}`, `{#each …}`, `{#match …}`, `{#snippet …}`: `rest` is what
    /// follows the `#`.
    fn block_open(&mut self, open: usize, rest: &str) -> Result<(), Error> {
        let (kw, arg) = split_word(rest);
        if arg.is_empty() {
            return Err(self.err(open, format!("{{#{kw}}} needs an expression")));
        }
        if kw == "snippet" {
            return self.snippet_open(open, arg);
        }
        let frame = match kw {
            "if" => Frame::If {
                pos: open,
                branches: vec![(self.code_at(open, arg), Vec::new())],
                otherwise: None,
            },
            "each" => {
                let (iter, pat, index) = split_each(arg).ok_or_else(|| {
                    self.err(
                        open,
                        "expected {#each <expr> as <pattern>[, <index>]}".into(),
                    )
                })?;
                Frame::Each {
                    pos: open,
                    iter: self.code_at(open, iter),
                    pat: pat.into(),
                    index: index.map(Into::into),
                    body: Vec::new(),
                    otherwise: None,
                }
            }
            "match" => Frame::Match {
                pos: open,
                scrutinee: self.code_at(open, arg),
                arms: Vec::new(),
            },
            _ => return Err(self.err(open, format!("unknown block {{#{kw}}}"))),
        };
        if matches!(self.ctx, Ctx::Quoted(_)) {
            self.in_value(open)?;
        }
        self.begin(open)?;
        self.open(frame);
        self.skip_standalone(open);
        Ok(())
    }

    /// `{/if}` and the like, server blocks' and client blocks' alike.
    fn block_close(&mut self, open: usize, kw: &str) -> Result<(), Error> {
        let open_kind = match self.frames.last() {
            Some(Frame::If { .. }) => "if",
            Some(Frame::Each { .. }) => "each",
            Some(Frame::Match { .. }) => "match",
            Some(Frame::Snippet { .. }) => "snippet",
            Some(Frame::Client { kind, .. }) if *kind != "comp" => block_of(kind),
            _ => "",
        };
        // `{/}` names no block, and so closes none, even with none open.
        if kw != open_kind || kw.is_empty() {
            return Err(self.unexpected_close(open, &format!("{{/{kw}}}")));
        }
        if matches!(self.frames.last(), Some(Frame::Client { .. })) {
            if matches!(kw, "await" | "try") {
                self.client_close(open)?;
            }
            self.client_close(open)?;
            self.skip_standalone(open);
            return Ok(());
        }
        let node = match self.end(open)? {
            Frame::If {
                branches,
                otherwise,
                ..
            } => Node::If {
                branches,
                otherwise,
            },
            Frame::Each {
                iter,
                pat,
                index,
                body,
                otherwise,
                ..
            } => Node::Each {
                iter,
                pat,
                index,
                body,
                otherwise,
            },
            Frame::Match {
                pos,
                scrutinee,
                arms,
            } => {
                if arms.is_empty() {
                    return Err(self.err(pos, "{#match} needs at least one {:case}".into()));
                }
                Node::Match { scrutinee, arms }
            }
            Frame::Snippet {
                pos,
                name,
                params,
                start,
                body,
            } => {
                self.snippets.push(Snip {
                    name: name.clone(),
                    params: params.clone(),
                    body: start..open,
                    depth: self.frames.len(),
                });
                Node::Snippet {
                    name,
                    params,
                    body,
                    line: self.line_of(pos),
                }
            }
            Frame::Head { .. } | Frame::Component { .. } | Frame::Client { .. } => {
                unreachable!("kw matched the open block")
            }
        };
        self.list().push(node);
        self.skip_standalone(open);
        Ok(())
    }

    /// `{@props …}`, `{@html …}`, `{@const …}`, `{@render …}`: `rest` is
    /// what follows the `@`; `unquoted`, it is an attribute's whole value.
    fn at_tag(&mut self, open: usize, rest: &str, unquoted: bool) -> Result<(), Error> {
        let (kw, arg) = split_word(rest);
        if kw == "props" {
            if self.ctx != Ctx::Text || !self.frames.is_empty() {
                return Err(self.err(
                    open,
                    "{@props …} goes at the top of a component, outside any tag or block".into(),
                ));
            }
            if self.props.is_some() {
                return Err(self.err(
                    open,
                    "a component declares its props once, in one {@props …}".into(),
                ));
            }
            let decls = parse_props(arg).map_err(|m| self.err(open, m))?;
            self.props = Some((decls, self.line_of(open)));
            self.skip_standalone(open);
            return Ok(());
        }
        let node = match kw {
            "html" if self.ctx != Ctx::Text => {
                return Err(self.err(open, "{@html} is not allowed inside tags".into()));
            }
            "html" if !arg.is_empty() => Node::Html(self.code_at(open, arg)),
            "const" if arg.contains('=') && !unquoted => Node::Const(self.code_at(open, arg)),
            "render" if arg.replace(' ', "") == "children()" && self.ctx == Ctx::Text => {
                self.uses_children = true;
                Node::Render
            }
            "render" if self.ctx == Ctx::Text => {
                let (name, args) = self.call(open, arg, "{@render name(arg, …)}")?;
                let local = match self.snippets.iter().rev().find(|s| s.name == name) {
                    Some(s) => {
                        self.check_args(open, &name, s.params.len(), &args)?;
                        true
                    }
                    None => false,
                };
                Node::RenderSnippet {
                    name,
                    args: self.code_at(open, &args),
                    local,
                }
            }
            _ => return Err(self.err(open, format!("unknown or malformed {{@{kw} …}}"))),
        };
        let silent = matches!(node, Node::Const(_));
        self.push_node(open, node)?;
        if silent {
            self.skip_standalone(open);
        }
        Ok(())
    }

    /// Takes ` name=` back off the end of the text: a boolean attribute's name
    /// is printed by its node, and only when its condition holds.
    fn unwrite_attr_name(&mut self, open: usize) -> Result<(), Error> {
        let t = self.text.strip_suffix('=').unwrap_or(&self.text).trim_end();
        let at = t
            .len()
            .checked_sub(self.attr.len())
            .filter(|&n| t.as_bytes()[n..].eq_ignore_ascii_case(self.attr.as_bytes()));
        let Some(at) = at else {
            return Err(self.err(
                open,
                format!("write {}={{condition}} in one piece", self.attr),
            ));
        };
        let keep = t[..at].trim_end().len();
        self.text.truncate(keep);
        Ok(())
    }

    /// A block tag alone on its line (Mustache calls it standalone) leaves no
    /// line behind: the whitespace after it is dropped, so `{#each}` and
    /// `{/each}` lines do not become blank lines in the output. The newline
    /// before the tag stays, so whitespace between rendered items does too.
    fn skip_standalone(&mut self, open: usize) {
        if self.ctx != Ctx::Text || self.preserve > 0 {
            return;
        }
        let inline = |c: &u8| matches!(c, b' ' | b'\t' | b'\r');
        let line_start = self.b[..open]
            .iter()
            .rposition(|c| !inline(c))
            .is_none_or(|p| self.b[p] == b'\n');
        let rest = &self.b[self.i..];
        let line_end = rest.iter().find(|c| !inline(c)).is_none_or(|&c| c == b'\n');
        if line_start && line_end {
            while self.i < self.b.len() && is_ws(self.b[self.i]) {
                self.i += 1;
            }
        }
    }

    // ---- errors -----------------------------------------------------------

    fn line_of(&self, pos: usize) -> u32 {
        self.line_starts.partition_point(|&s| s <= pos) as u32
    }

    fn col_of(&self, pos: usize) -> u32 {
        self.src[self.line_starts[self.line_of(pos) as usize - 1]..pos]
            .chars()
            .count() as u32
            + 1
    }

    fn err(&self, pos: usize, msg: String) -> Error {
        Error {
            line: self.line_of(pos),
            col: self.col_of(pos),
            msg,
        }
    }
}

/// An `<option>`'s `selected={…}`, by its `<select>`'s choice
/// (`Node::Chosen`).
pub const IS: &str = "::wisp::rt::is(&__wisp_sel, ";

/// Text of the page as the browser reads it: character references
/// decoded (the ones `escape` writes, `&apos;`, `&nbsp;` and numeric ones);
/// any other `&` stays.
fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let c = end.and_then(|e| match &rest[1..e] {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            n => {
                let n = n.strip_prefix('#')?;
                let code = match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse().ok(),
                };
                code.and_then(char::from_u32)
            }
        });
        match (c, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `cx.problem("x")` as a whole hole, its name any string literal: the
/// field's name.
fn problem_call(t: &str) -> Option<String> {
    let arg = t.trim().strip_prefix("cx.problem(")?.strip_suffix(')')?;
    match crate::fold::literal(arg)? {
        crate::fold::Lit::Str(name) => Some(name),
        _ => None,
    }
}

/// Leaves out of `list` (and the lists in it) each automatic problem of a
/// field the file shows itself, joining the text around it.
fn drop_shown(list: &mut Vec<Node>, chunks: &mut [String], shown: &[String]) {
    let mut k = 0;
    while k < list.len() {
        let shown_here =
            matches!(&list[k], Node::Problem { name, auto: true, .. } if shown.contains(name));
        // (Lists alternate text and nodes, so text is on both sides.)
        if let (true, Some(&Node::Text(a)), Some(&Node::Text(b))) = (
            shown_here,
            k.checked_sub(1).map(|j| &list[j]),
            list.get(k + 1),
        ) {
            let after = std::mem::take(&mut chunks[b]);
            chunks[a].push_str(&after);
            list.drain(k..k + 2);
            continue;
        }
        let mut go = |l: &mut Vec<Node>| drop_shown(l, chunks, shown);
        match &mut list[k] {
            Node::Snippet { body, .. } | Node::Head(body) => go(body),
            Node::If {
                branches,
                otherwise,
            } => {
                branches.iter_mut().for_each(|(_, l)| go(l));
                otherwise.iter_mut().for_each(go);
            }
            Node::Each {
                body, otherwise, ..
            } => {
                go(body);
                otherwise.iter_mut().for_each(go);
            }
            Node::Match { arms, .. } => arms.iter_mut().for_each(|(_, l)| go(l)),
            Node::Component { children, .. } => children.iter_mut().for_each(go),
            Node::Kept { sent, own, .. } => {
                go(sent);
                own.iter_mut().for_each(go);
            }
            _ => {}
        }
        k += 1;
    }
}

/// The block a client frame's kind is written as: an await's or try's
/// branches are theirs.
fn block_of(kind: &str) -> &str {
    match kind {
        "pending" | "then" | "catch" => "await",
        "body" | "failed" => "try",
        k => k,
    }
}

/// An attribute that is browser code rather than HTML: `on:click`,
/// `bind:value`, `:hidden`, `class:open`, `style:--x`, `transition:fade`,
/// `use:tooltip`, and `each` or `if` on a `<template>`.
fn is_directive(raw: &str, tag: &str) -> bool {
    raw.starts_with(':')
        || [
            "on:",
            "bind:",
            "class:",
            "style:",
            "transition:",
            "in:",
            "out:",
            "use:",
            "animate:",
            "client:",
        ]
        .iter()
        .any(|p| raw.starts_with(p))
        || (tag == "template"
            && (raw.eq_ignore_ascii_case("each") || raw.eq_ignore_ascii_case("if")))
}

/// A directive's kind, name and modifiers, checked, from its attribute name.
fn directive_parts(raw: &str, tag: &str) -> Result<(Dir, String, Vec<String>), String> {
    if tag == "template" && raw.eq_ignore_ascii_case("each") {
        return Ok((Dir::Each, String::new(), Vec::new()));
    }
    if tag == "template" && raw.eq_ignore_ascii_case("if") {
        return Ok((Dir::If, String::new(), Vec::new()));
    }
    if let Some(start) = raw.strip_prefix("client:") {
        let Some(short) = how(start) else {
            return Err(format!(
                "`{raw}` is not a way to start: client:load (the default), client:visible, client:idle, client:interaction, client:media=\"(query)\" or client:none"
            ));
        };
        return Ok((Dir::Wait, short.into(), Vec::new()));
    }
    let (kind, name) = if let Some(n) = raw.strip_prefix("on:") {
        let mut parts = n.split('.');
        let event = parts.next().unwrap_or("");
        if event.is_empty() {
            return Err(format!("`{raw}` needs an event's name, such as on:click"));
        }
        let mods: Vec<String> = parts.map(String::from).collect();
        check_modifiers(&mods)?;
        return Ok((Dir::On, event.to_string(), mods));
    } else if let Some(n) = raw.strip_prefix("bind:") {
        if !is_ident(n) {
            return Err(format!(
                "`{raw}` binds a property of the element: bind:value, bind:checked, bind:group, bind:files, bind:this, bind:clientWidth, …"
            ));
        }
        (Dir::Bind, n)
    } else if let Some(n) = raw.strip_prefix("class:") {
        (Dir::Class, n)
    } else if let Some(n) = raw.strip_prefix("style:") {
        (Dir::Style, n)
    } else if let Some((dir, n)) = ["transition:", "in:", "out:"]
        .iter()
        .find_map(|p| Some((&p[..p.len() - 1], raw.strip_prefix(p)?)))
    {
        // A built-in (fade, slide, scale, fly, blur) or a script function.
        if !is_ident(n) {
            return Err(format!(
                "`{raw}` needs a transition: fade, slide, scale, fly, blur, or a function of the script, such as {dir}:spin"
            ));
        }
        let mods = if dir == "transition" {
            Vec::new()
        } else {
            vec![dir.to_string()]
        };
        return Ok((Dir::Transition, n.to_string(), mods));
    } else if let Some(n) = raw.strip_prefix("animate:") {
        if n != "flip" {
            return Err(format!(
                "`{raw}` is not an animation: use animate:flip, on an element of a keyed {{:#each}}"
            ));
        }
        (Dir::Animate, n)
    } else if let Some(n) = raw.strip_prefix("use:") {
        let ident = n
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_' || c == b'$')
            && n.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$');
        if !ident {
            return Err(format!(
                "`{raw}` needs the name of a function in the script, such as use:tooltip"
            ));
        }
        (Dir::Use, n)
    } else {
        let n = raw.strip_prefix(':').unwrap_or(raw);
        (
            if n.eq_ignore_ascii_case("text") {
                Dir::Text
            } else {
                Dir::Attr
            },
            n,
        )
    };
    if name.is_empty() {
        return Err(format!("`{raw}` needs a name after the `:`"));
    }
    Ok((kind, name.to_string(), Vec::new()))
}

/// An island's start, `client:<start>`, as a record's `how` says it
/// (`protocol::ISLAND_*`): "" for `load`, the default, which is no
/// island; `None` for no way to start. `media`'s query follows it.
pub fn how(start: &str) -> Option<&'static str> {
    Some(match start {
        "load" => "",
        "visible" => ISLAND_VISIBLE,
        "idle" => ISLAND_IDLE,
        "interaction" => ISLAND_INTERACTION,
        "none" => ISLAND_NONE,
        "media" => ISLAND_MEDIA,
        _ => return None,
    })
}

const KEY_NAMES: [&str; 14] = [
    "enter",
    "escape",
    "space",
    "tab",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
];
/// The keys held down: the last of `ON_FLAGS`.
const MODIFIER_KEYS: [&str; 4] = ["ctrl", "shift", "alt", "meta"];

/// An event's modifiers: `ON_FLAGS` (which live.js takes as bits),
/// `debounce` with a duration after it if it likes, and keys.
fn check_modifiers(mods: &[String]) -> Result<(), String> {
    let duration = |d: &str| {
        let digits = d
            .strip_suffix("ms")
            .or_else(|| d.strip_suffix('s'))
            .unwrap_or("");
        !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit())
    };
    let mut k = 0;
    while k < mods.len() {
        let m = mods[k].as_str();
        if m == "debounce" && mods.get(k + 1).is_some_and(|d| duration(d)) {
            k += 2;
            continue;
        }
        let one_key = m.len() == 1
            && m.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        if !(ON_FLAGS.contains(&m) || m == "debounce" || KEY_NAMES.contains(&m) || one_key) {
            let modifiers: Vec<&str> = (ON_FLAGS.iter().copied())
                .filter(|f| !MODIFIER_KEYS.contains(f))
                .chain(["debounce"])
                .collect();
            return Err(format!(
                "`.{m}` is not an event modifier. Use {}, with a time after debounce if you like (debounce.300ms); \
                 a key: {}, or one letter or digit; or a key held down: {}",
                modifiers.join(", "),
                KEY_NAMES.join(", "),
                MODIFIER_KEYS.join(", ")
            ));
        }
        k += 1;
    }
    Ok(())
}

/// True if `code` has a `//` comment outside its string and char literals.
fn has_line_comment(code: &str) -> bool {
    let b = code.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => return true,
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            _ => {}
        }
        i += 1;
    }
    false
}

/// HTML's boolean attributes: present means on, whatever the value.
pub(crate) const BOOLEAN_ATTRS: [&str; 25] = [
    "allowfullscreen",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "controls",
    "default",
    "defer",
    "disabled",
    "formnovalidate",
    "hidden",
    "inert",
    "ismap",
    "itemscope",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "playsinline",
    "readonly",
    "required",
    "reversed",
    "selected",
];

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

/// Splits `"kw rest"` into `("kw", "rest")`.
fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(|c: char| c.is_whitespace()) {
        Some(n) => (&s[..n], s[n..].trim()),
        None => (s, ""),
    }
}

/// `iter as pat[, index]`: the *last* top-level `as` separates, since the
/// iterator expression may contain casts and the pattern never can.
fn split_each(arg: &str) -> Option<(&str, &str, Option<&str>)> {
    let b = arg.as_bytes();
    let mut at = None;
    for_each_top(arg, |i| {
        let word = b[i..].starts_with(b"as")
            && i > 0
            && is_ws(b[i - 1])
            && b.get(i + 2).is_some_and(|&c| is_ws(c));
        if word {
            at = Some(i);
        }
    });
    let at = at?;
    let (iter, rest) = (arg[..at].trim(), arg[at + 2..].trim());
    let mut comma = None;
    for_each_top(rest, |i| {
        if rest.as_bytes()[i] == b',' {
            comma = Some(i);
        }
    });
    let (pat, index) = match comma {
        Some(c) => (rest[..c].trim(), Some(rest[c + 1..].trim())),
        None => (rest, None),
    };
    if iter.is_empty() || pat.is_empty() || index.is_some_and(|i| !is_ident(i)) {
        return None;
    }
    Some((iter, pat, index))
}

/// `list as item, i (key)`: the list, the pattern and the key. The last
/// top-level ` as ` separates; the key is a parenthesized tail after it.
fn split_client_each(arg: &str) -> Option<(&str, &str, Option<&str>)> {
    let at = arg.rfind(" as ")?;
    let (list, mut pat) = (arg[..at].trim(), arg[at + 4..].trim());
    let mut key = None;
    if pat.ends_with(')') {
        let open = pat.find('(')?;
        key = Some(pat[open + 1..pat.len() - 1].trim()).filter(|k| !k.is_empty());
        key?;
        pat = pat[..open].trim();
    }
    (!list.is_empty() && !pat.is_empty()).then_some((list, pat, key))
}

/// Markup the browser can render by itself: static text and browser code,
/// with no server expression or block.
pub fn client_renderable(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| match n {
        // A snippet's definition draws nothing; `{:@render}` copied it here.
        Node::Text(_)
        | Node::Live { .. }
        | Node::Hole { .. }
        | Node::Tag { .. }
        | Node::Render
        | Node::Snippet { .. } => true,
        Node::Client(branches) => branches.iter().all(|(_, b)| client_renderable(b)),
        _ => false,
    })
}

fn prop_shape(props: &[Prop], out: &mut Vec<u8>) {
    for p in props {
        out.extend_from_slice(p.name.as_bytes());
        let (tag, s) = match &p.value {
            PropValue::Expr(c) => (b'=', c.src.as_str()),
            PropValue::Text(t) => (b'"', t.as_str()),
            PropValue::Flag => (b'!', ""),
            PropValue::Live(c) => (b':', c.src.as_str()),
            PropValue::Bind(c) => (b'b', c.src.as_str()),
            PropValue::On(c) => (b'o', c.src.as_str()),
            PropValue::Snippet { name, .. } => (b's', name.as_str()),
        };
        out.push(tag);
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
}

/// A component's tag, and so its file name: a capital letter first, a
/// lowercase letter somewhere (an all-capitals tag is HTML), and only
/// letters, digits and `_`.
pub fn is_component_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.first().is_some_and(u8::is_ascii_uppercase)
        && b.iter().any(u8::is_ascii_lowercase)
        && b.iter().all(|&c| is_word(c))
}

/// A pattern without its type: `item: &Item` → `item`.
pub(crate) fn untyped(pat: &str) -> &str {
    let b = pat.as_bytes();
    let mut colon = None;
    for_each_top(pat, |i| {
        let single = b[i] == b':' && b.get(i + 1) != Some(&b':') && (i == 0 || b[i - 1] != b':');
        if single && colon.is_none() {
            colon = Some(i);
        }
    });
    pat[..colon.unwrap_or(pat.len())].trim()
}

/// A snippet prop's type: `Snippet<&Row, usize>` is a function that
/// renders one, `&dyn Fn(&mut Out, &Row, usize)`.
fn snippet_type(ty: &str) -> String {
    let args = if ty == "Snippet" {
        Some("")
    } else {
        ty.strip_prefix("Snippet<")
            .and_then(|s| s.strip_suffix('>'))
    };
    match args.map(str::trim) {
        Some("") => "&dyn Fn(&mut ::wisp::Out)".into(),
        Some(a) => format!("&dyn Fn(&mut ::wisp::Out, {a})"),
        None => ty.to_string(),
    }
}

/// `title: &str, size: u8 = 2` → the props a component declares.
fn parse_props(arg: &str) -> Result<Vec<PropDecl>, String> {
    let mut out: Vec<PropDecl> = Vec::new();
    for part in split_top(arg, b',') {
        let part = part.trim();
        if part.is_empty() {
            continue; // a trailing comma
        }
        let (name, rest) = part
            .split_once(':')
            .ok_or_else(|| format!("expected `name: Type` in {{@props …}}, found `{part}`"))?;
        let name = name.trim();
        if !is_ident(name) {
            return Err(format!("`{name}` is not a name for a prop"));
        }
        if name == "children" || name.starts_with("__") {
            return Err(format!(
                "`{name}` is a name Wisp uses; call the prop something else"
            ));
        }
        let (ty, default) = match split_top(rest, b'=').as_slice() {
            [ty] => (ty.trim(), None),
            [ty, default] if !default.trim().is_empty() => {
                (ty.trim(), Some(default.trim().to_string()))
            }
            _ => {
                return Err(format!(
                    "expected `{name}: Type` or `{name}: Type = default` in {{@props …}}"
                ));
            }
        };
        if ty.is_empty() {
            return Err(format!("prop `{name}` needs a type: `{name}: &str`"));
        }
        if out.iter().any(|d| d.name == name) {
            return Err(format!("prop `{name}` is declared twice"));
        }
        out.push(PropDecl {
            name: name.to_string(),
            ty: snippet_type(ty),
            default,
        });
    }
    if out.is_empty() {
        return Err(
            "{@props …} lists the component's props: {@props title: &str, count: u32 = 0}".into(),
        );
    }
    Ok(out)
}

/// Splits `s` at each `sep` that is outside brackets (`<>` too, since types
/// have them), strings and chars. `=` never splits `==`, `=>`, `<=`, `>=`
/// or `!=`, and `->` is not a bracket.
fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut depth, mut from, mut i) = (0i32, 0, 0);
    let mut parts = Vec::new();
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b'>' if i > 0 && matches!(b[i - 1], b'-' | b'=') => {}
            b')' | b']' | b'}' | b'>' => depth -= 1,
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            c if c == sep && depth == 0 => {
                let paired = sep == b'='
                    && (b.get(i + 1).is_some_and(|&n| n == b'=' || n == b'>')
                        || (i > 0 && matches!(b[i - 1], b'=' | b'!' | b'<' | b'>')));
                if !paired {
                    parts.push(&s[from..i]);
                    from = i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[from.min(s.len())..]);
    parts
}

/// Calls `f` with every byte index at bracket depth 0 that is outside string
/// and char literals.
pub(crate) fn for_each_top(s: &str, mut f: impl FnMut(usize)) {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            _ if depth == 0 => f(i),
            _ => {}
        }
        i += 1;
    }
}

/// Index of the `}` closing a hole that starts at `i`, skipping nested braces
/// and Rust string/char literals.
fn hole_end(b: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 0u32;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            _ => {}
        }
        i += 1;
    }
    None
}

pub(crate) use wisp_shared::rust::{raw_str_start, skip_char, skip_raw_str, skip_str};

/// Serializes everything but static text, for the shape hash.
fn shape(nodes: &[Node], out: &mut Vec<u8>) {
    let code = |out: &mut Vec<u8>, c: &Code| {
        out.extend_from_slice(c.src.as_bytes());
        out.push(0);
    };
    for n in nodes {
        match n {
            Node::Text(_) => out.push(b'T'),
            Node::Expr(c) => {
                out.push(b'E');
                code(out, c);
            }
            Node::UrlStart { prefix } => {
                out.push(b'U');
                out.extend_from_slice(prefix.as_bytes());
                out.push(0);
            }
            Node::UrlEnd => out.push(b'u'),
            Node::Bool {
                name,
                code: c,
                class,
            } => {
                out.push(if *class { b'b' } else { b'B' });
                out.extend_from_slice(name.as_bytes());
                out.push(0);
                code(out, c);
            }
            Node::Attr { name, code: c, url } => {
                out.push(b'A');
                out.extend_from_slice(name.as_bytes());
                out.push(*url as u8);
                code(out, c);
            }
            Node::Html(c) => {
                out.push(b'H');
                code(out, c);
            }
            Node::Const(c) => {
                out.push(b'C');
                code(out, c);
            }
            Node::Render => out.push(b'R'),
            Node::Snippet {
                name, params, body, ..
            } => {
                out.push(b'S');
                out.extend_from_slice(name.as_bytes());
                out.push(0);
                out.extend_from_slice(params.join(",").as_bytes());
                out.push(0);
                shape(body, out);
                out.push(b'.');
            }
            Node::RenderSnippet { name, args, local } => {
                out.push(if *local { b'r' } else { b'p' });
                out.extend_from_slice(name.as_bytes());
                out.push(0);
                code(out, args);
            }
            Node::If {
                branches,
                otherwise,
            } => {
                out.push(b'I');
                for (c, body) in branches {
                    code(out, c);
                    shape(body, out);
                    out.push(b';');
                }
                if let Some(o) = otherwise {
                    out.push(b'!');
                    shape(o, out);
                }
                out.push(b'.');
            }
            Node::Each {
                iter,
                pat,
                index,
                body,
                otherwise,
            } => {
                out.push(b'L');
                code(out, iter);
                out.extend_from_slice(pat.as_bytes());
                out.push(0);
                out.extend_from_slice(index.as_deref().unwrap_or("").as_bytes());
                out.push(0);
                shape(body, out);
                if let Some(o) = otherwise {
                    out.push(b'!');
                    shape(o, out);
                }
                out.push(b'.');
            }
            Node::Match { scrutinee, arms } => {
                out.push(b'M');
                code(out, scrutinee);
                for (c, body) in arms {
                    code(out, c);
                    shape(body, out);
                    out.push(b';');
                }
                out.push(b'.');
            }
            Node::Head(body) => {
                out.push(b'h');
                shape(body, out);
                out.push(b'.');
            }
            Node::Kept {
                name, sent, own, ..
            } => {
                out.push(b'k');
                out.extend_from_slice(name.as_bytes());
                out.push(0);
                shape(sent, out);
                out.push(b';');
                if let Some(own) = own {
                    shape(own, out);
                }
                out.push(b'.');
            }
            Node::Chosen { name, own, .. } => {
                out.push(b'c');
                out.extend_from_slice(name.as_bytes());
                out.push(0);
                out.extend_from_slice(own.as_deref().unwrap_or("").as_bytes());
                out.push(0);
            }
            Node::Problem { name, .. } => {
                out.push(b'P');
                out.extend_from_slice(name.as_bytes());
                out.push(0);
            }
            Node::Component {
                name,
                props,
                children,
                ..
            } => {
                out.push(b'K');
                out.extend_from_slice(name.as_bytes());
                out.push(0);
                prop_shape(props, out);
                if let Some(c) = children {
                    out.push(b'!');
                    shape(c, out);
                }
                out.push(b'.');
            }
            Node::Live { group } | Node::Hole { group } | Node::Tag { group } => {
                out.push(match n {
                    Node::Live { .. } => b'W',
                    Node::Hole { .. } => b'O',
                    _ => b'N',
                });
                out.extend_from_slice(group.to_string().as_bytes());
                out.push(0);
            }
            Node::Client(branches) => {
                out.push(b'F');
                for (group, body) in branches {
                    out.extend_from_slice(group.to_string().as_bytes());
                    out.push(0);
                    shape(body, out);
                    out.push(b';');
                }
                out.push(b'.');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &Template, n: &Node) -> String {
        match n {
            Node::Text(i) => t.chunks[*i].clone(),
            _ => panic!("not text: {n:?}"),
        }
    }

    fn assert_alternates(nodes: &[Node]) {
        assert!(nodes.len() % 2 == 1, "odd length: {nodes:?}");
        for (i, n) in nodes.iter().enumerate() {
            assert_eq!(matches!(n, Node::Text(_)), i % 2 == 0, "{nodes:?}");
            match n {
                Node::If {
                    branches,
                    otherwise,
                } => {
                    branches.iter().for_each(|b| assert_alternates(&b.1));
                    otherwise.iter().for_each(|o| assert_alternates(o));
                }
                Node::Each {
                    body, otherwise, ..
                } => {
                    assert_alternates(body);
                    otherwise.iter().for_each(|o| assert_alternates(o));
                }
                Node::Kept { sent, own, .. } => {
                    assert_alternates(sent);
                    own.iter().for_each(|o| assert_alternates(o));
                }
                Node::Match { arms, .. } => arms.iter().for_each(|a| assert_alternates(&a.1)),
                Node::Head(b) => assert_alternates(b),
                Node::Component {
                    children: Some(c), ..
                } => assert_alternates(c),
                _ => {}
            }
        }
    }

    #[test]
    fn components() {
        let t = parse("<ul>{#each xs as x}<Item name={x.name} big label=\"a b\" >\n  <b>{x}</b>\n</Item>{/each}<Divider/></ul>").unwrap();
        assert_alternates(&t.nodes);
        let Node::Each { body, .. } = &t.nodes[1] else {
            panic!("{:?}", t.nodes)
        };
        let Node::Component {
            name,
            props,
            children: Some(children),
            line,
        } = &body[1]
        else {
            panic!("{body:?}")
        };
        assert_eq!((name.as_str(), *line), ("Item", 1));
        let props: Vec<_> = props
            .iter()
            .map(|p| (p.name.as_str(), p.value.clone()))
            .collect();
        let code = Code {
            src: "x.name".into(),
            line: 1,
        };
        assert_eq!(
            props,
            [
                ("name", PropValue::Expr(code)),
                ("big", PropValue::Flag),
                ("label", PropValue::Text("a b".into()))
            ]
        );
        assert!(matches!(&children[1], Node::Expr(c) if c.src == "x"));
        assert!(
            matches!(&t.nodes[3], Node::Component { name, children: None, .. } if name == "Divider")
        );
        // Lowercase tags are HTML, whatever their case in the source.
        assert_eq!(parse("<Div-x>a</Div-x>").unwrap().nodes.len(), 1);
    }

    #[test]
    fn component_props() {
        let t = parse("{@props title: &str, tags: HashMap<String, Vec<u8>> = HashMap::new(), on: bool = a == b,}\n<h2>{title}</h2>").unwrap();
        let (props, line) = t.props.as_ref().unwrap();
        let decls: Vec<_> = props
            .iter()
            .map(|d| (d.name.as_str(), d.ty.as_str(), d.default.as_deref()))
            .collect();
        assert_eq!(
            decls,
            [
                ("title", "&str", None),
                ("tags", "HashMap<String, Vec<u8>>", Some("HashMap::new()")),
                ("on", "bool", Some("a == b"))
            ]
        );
        assert_eq!(*line, 1);
        assert_eq!(text(&t, &t.nodes[0]), "<h2>");
        let f = parse("{@props f: &dyn Fn(u8) -> u8}")
            .unwrap()
            .props
            .unwrap()
            .0;
        assert_eq!(f[0].ty, "&dyn Fn(u8) -> u8");

        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("{@props}").contains("lists the component's props"));
        assert!(err("{@props a}").contains("expected `name: Type`"));
        assert!(err("{@props a: u8, a: u8}").contains("declared twice"));
        assert!(err("{@props children: u8}").contains("a name Wisp uses"));
        assert!(err("{@props a: u8}{@props b: u8}").contains("once"));
        assert!(err("{#if x}{@props a: u8}{/if}").contains("at the top"));
    }

    #[test]
    fn component_errors() {
        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(
            err("<Card>x</div>").contains("never closed"),
            "{}",
            err("<Card>x</div>")
        );
        assert!(err("<Card>{#if a}</Card>{/if}").contains("does not match the {#if}"));
        assert!(err("<Card>x</Other>").contains("does not match the <Card>"));
        assert!(err("<Card title=\"a {b}\" />").contains("plain text"));
        assert!(err("<Card title= />").contains("needs a value"));
        assert!(err("<Card a a />").contains("twice"));
        assert!(err("<Card {x + 1} />").contains("one name in the braces"));
        assert!(err("<Card 1 />").contains("takes props"));
        assert!(err("<Card").contains("unclosed <Card>"));
    }

    #[test]
    fn plain_text() {
        let t = parse("  <p>hi</p>\n").unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(text(&t, &t.nodes[0]), "<p>hi</p>");
    }

    #[test]
    fn expr_and_whitespace() {
        let t = parse("<ul>\n    <li>{a.b}</li>\n  </ul>").unwrap();
        assert_alternates(&t.nodes);
        assert_eq!(text(&t, &t.nodes[0]), "<ul>\n<li>");
        assert!(matches!(&t.nodes[1], Node::Expr(code) if code.src == "a.b" && code.line == 2));
        assert_eq!(text(&t, &t.nodes[2]), "</li>\n</ul>");
    }

    #[test]
    fn braces_in_rust_literals() {
        let t = parse(r#"{format!("{}}", x)}{'}'}{r"}"}"#).unwrap();
        assert_alternates(&t.nodes);
        let exprs: Vec<_> = t
            .nodes
            .iter()
            .filter_map(|n| {
                if let Node::Expr(code) = n {
                    Some(code.src.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(exprs, [r#"format!("{}}", x)"#, "'}'", r#"r"}""#]);
    }

    #[test]
    fn attribute_values() {
        let t = parse(r#"<a href="/p/{id}" class={cls} data-x='{y}' title= {z}>"#).unwrap();
        assert_alternates(&t.nodes);
        let texts: Vec<_> = t.nodes.iter().step_by(2).map(|n| text(&t, n)).collect();
        assert_eq!(
            texts,
            [
                r#"<a href="/p/"#,
                r#"""#,
                r#" data-x='"#,
                r#"' title= ""#,
                r#"">"#
            ]
        );
    }

    #[test]
    fn rejects_unsafe_contexts() {
        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("<div {x.y}>").contains("attribute values"));
        assert!(err(r#"<a onclick="go({x})">"#).contains("event handler"));
        assert!(err("<a onclick={x}>").contains("event handler"));
        assert!(parse(r#"<a title="{@html x}">"#).is_err());
        assert!(err("<p><{x}></p>").contains("tag's name"));
        assert!(err("<p></{x}></p>").contains("tag's name"));
        assert!(err("<iframe srcdoc={x}>").contains("srcdoc"));
        // In any case, and as `{name}` for `name={name}`.
        assert!(err("<a ONCLICK={x}>").contains("event handler"));
        assert!(err("<a {ONCLICK}>").contains("event handler"));
        assert!(err("<a {OnMouseOver}>").contains("event handler"));
        assert!(err("<iframe {SRCDOC}>").contains("srcdoc"));
        assert!(err("<iframe SrcDoc={x}>").contains("srcdoc"));
        assert!(err("<svg><SET {TO} /></svg>").contains("`to`"));
        assert!(err(r#"<meta {CONTENT} HTTP-EQUIV="refresh">"#).contains("refresh"));
        // Browser values too: `:attr`, `attr={:…}`, in any case.
        assert!(err(r#"<a :ONCLICK="x">"#).contains("run script"));
        assert!(err(r#"<iframe :srcdoc="x">"#).contains("run script"));
        assert!(err(r#"<svg><set :to="x" /></svg>"#).contains("run script"));
        assert!(err(r#"<meta http-equiv="refresh" :CONTENT="x">"#).contains("refresh"));
        assert!(err("<a OnClick={:x}>").contains("no {:…}"));
        assert!(parse(r#"<meta name="description" :content="x">"#).is_ok());
        // A spread knows its tag, for the keys it leaves out (`js_attrs`).
        let t = parse("<META {:...a}>").unwrap();
        assert_eq!(t.groups[0].directives[0].name, "meta");
        assert!(err(r#"<a href="javascript:go('{x}')">"#).contains("runs script"));
        assert!(err(r#"<a href=" JavaScript:{x}">"#).contains("runs script"));
        assert!(err(r#"<a href="java&#115;cript:{x}">"#).contains("character reference"));
        assert!(err("<p>{x // why}</p>").contains("// comments"));
        // URLs that are not in URL attributes.
        assert!(err(r#"<svg><a><set attributeName="href" to={x} /></a></svg>"#).contains("`to`"));
        assert!(err(r#"<svg><animate values="0;{x}" /></svg>"#).contains("`values`"));
        assert!(err(r#"<meta content={x} http-equiv="Refresh">"#).contains("refresh"));
        assert!(err(r#"<meta http-equiv="refresh" content="0;url={x}">"#).contains("refresh"));
        assert!(err("<meta http-equiv={x} content=\"0\">").contains("http-equiv"));
        assert!(parse(r#"<meta name="description" content={x}>"#).is_ok());
        assert!(parse(r#"<svg><animate attributeName="r" values="0;9" /></svg>"#).is_ok());
        // Live values too, and a static start that runs script.
        assert!(parse(r#"<svg><set to={:x} /></svg>"#).is_err());
        assert!(parse(r#"<meta http-equiv="refresh" content={:x}>"#).is_err());
        assert!(err(r#"<a href="javascript:{:x}">"#).contains("runs script"));
        assert!(parse(r#"<a href="/p/{:x}">"#).is_ok());
    }

    #[test]
    fn a_bare_less_than_cannot_start_a_tag() {
        let t = parse("<p>a < b, &lt;{x}</p>").unwrap();
        assert_eq!(text(&t, &t.nodes[0]), "<p>a &lt; b, &lt;");
        let t = parse("<p>1 <2</p>").unwrap();
        assert_eq!(text(&t, &t.nodes[0]), "<p>1 &lt;2</p>");
    }

    #[test]
    fn url_values_whose_scheme_is_an_expression_are_guarded() {
        let kinds = |src: &str| -> String {
            let t = parse(src).unwrap();
            assert_alternates(&t.nodes);
            t.nodes
                .iter()
                .filter_map(|n| match n {
                    Node::UrlStart { prefix } => Some(format!("[{prefix}")),
                    Node::UrlEnd => Some("]".into()),
                    Node::Attr { code, url, .. } if *url => Some(format!("[{}]", code.src)),
                    Node::Attr { code, .. } | Node::Expr(code) => Some(code.src.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(kinds("<a href={u}>"), "[u]");
        assert_eq!(kinds(r#"<a href="{a}{b}" title={c}>"#), "[ab]c");
        assert_eq!(kinds(r#"<img src='java{x}'>"#), "[javax]");
        assert_eq!(kinds(r#"<form action="{#if a}{x}{/if}">"#), "[]"); // x is inside the {#if}
        // A static start that fixes the scheme needs no guard.
        for fixed in [
            r#"<a href="/p/{id}">"#,
            r#"<a href="https://x.com/{p}">"#,
            r#"<a href="?q={q}">"#,
            r#"<a href="mailto:{m}">"#,
        ] {
            assert_eq!(
                kinds(fixed).len(),
                kinds(fixed).trim_matches(['[', ']']).len(),
                "{fixed}"
            );
        }
    }

    #[test]
    fn blocks_end_where_they_began() {
        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err(r#"<div {#if a}class="on">{/if}{x}</div>"#).contains("same place"));
        assert!(err(r#"<a {#if a}title={:else}{/if}{y}>"#).contains("same place"));
        assert!(err(r#"<a title="{#if a}x"{/if}>"#).contains("same place"));
        assert!(err("{#if a}<p {:else}<b>{/if}>").contains("same place"));
        assert!(parse(r#"<a {#if a}class="x"{:else}title={t}{/if} href="/">"#).is_ok());
        assert!(parse(r#"<a title="{#if a}x{:else}{y}{/if}">"#).is_ok());
    }

    #[test]
    fn helpful_errors() {
        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("{#if a}x{:elseif b}y{/if}").contains("{:else if"));
        assert!(
            err("{#each xs as x}
{/if}")
            .contains("{#each} still open from line 1")
        );
        assert!(err("<p>use { to</p>").contains(r#"{"{"}"#));
        assert!(err("{#each xs as x, 0}{/each}").contains("expected {#each"));
        assert!(err("{/if}").contains("no block to close"));
    }

    #[test]
    fn empty_comments_and_nomodule() {
        let t = parse("<!-->shown<!--->too<!-- {x} -->").unwrap();
        assert_eq!(text(&t, &t.nodes[0]), "showntoo");
        let t = parse("<script nomodule>old()</script>").unwrap();
        assert_eq!(text(&t, &t.nodes[0]), "<script nomodule>old()</script>");
    }

    #[test]
    fn script_style_comment_are_raw() {
        let t = parse(
            "<style>a { color: red }</style><script defer>if (a) { b() }</script><!-- {x} -->done",
        )
        .unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(
            text(&t, &t.nodes[0]),
            "<style>a { color: red }</style><script defer type=\"module\">if (a) { b() }</script>done"
        );
    }

    #[test]
    fn a_bare_script_is_the_client_script() {
        let t = parse("<p>hi</p>\n\n<SCRIPT>\n  let a = '</p>' // {x}\n</SCRIPT >\n").unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(text(&t, &t.nodes[0]), "<p>hi</p>");
        assert_eq!(
            t.script,
            Some(Script {
                src: "\n  let a = '</p>' // {x}\n".into(),
                line: 3,
                col: 9
            })
        );
        assert!(t.is_live());

        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("<script>a</script><script>b</script>").contains("one client script"));
        assert!(err("{#if x}<script>a</script>{/if}").contains("top level"));
        assert!(err("<wisp:head><script>a</script></wisp:head>").contains("top level"));
        assert!(err("<script>a").contains("unclosed <script>"));
    }

    #[test]
    fn directives_leave_the_html() {
        let t = parse("<button class=\"b\"\n  on:click.prevent=\"open = !open\" :aria-expanded='open' title={t}\n  class:on=\" on \">Go</button>").unwrap();
        assert_alternates(&t.nodes);
        let texts: Vec<_> = t.nodes.iter().step_by(2).map(|n| text(&t, n)).collect();
        assert_eq!(texts, ["<button class=\"b\"", "", ">Go</button>"]);
        assert!(matches!(t.nodes[3], Node::Live { group: 0 }));
        let g = &t.groups[0];
        assert_eq!((g.nested, g.line, g.locals.len()), (false, 1, 0));
        let got: Vec<_> = g
            .directives
            .iter()
            .map(|d| {
                (
                    d.kind,
                    d.name.as_str(),
                    d.mods.join("."),
                    d.value.as_ref().map(|v| (v.src.as_str(), v.line)),
                    d.line,
                    d.col,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    Dir::On,
                    "click",
                    "prevent".into(),
                    Some(("open = !open", 2)),
                    2,
                    3
                ),
                (
                    Dir::Attr,
                    "aria-expanded",
                    String::new(),
                    Some(("open", 2)),
                    2,
                    35
                ),
                (Dir::Class, "on", String::new(), Some(("on", 3)), 3, 3),
            ]
        );

        // `/>` stays last; bare transitions and uses need no value.
        let t = parse("<input bind:value=\"q\" transition:fade use:focus />").unwrap();
        assert_eq!(text(&t, &t.nodes[0]), "<input");
        assert_eq!(text(&t, &t.nodes[2]), "/>");
        assert_eq!(
            t.groups[0]
                .directives
                .iter()
                .map(|d| d.value.is_some())
                .collect::<Vec<_>>(),
            [true, false, false]
        );
        // Keys, a debounce time and held keys are modifiers.
        let t = parse("<input on:keydown.ctrl.enter.prevent=\"go\" on:input.debounce.300ms=\"s\" on:keyup.k.window=\"k\">").unwrap();
        assert_eq!(t.groups[0].directives[1].mods, ["debounce", "300ms"]);
    }

    #[test]
    fn client_templates() {
        let t = parse("<ul><template each=\"item, i in data.list\"><li :text=\"item.name\"></li><template if=\"i\"><b :text=\"i\"></b></template></template></ul><p :hidden=\"x\"></p>").unwrap();
        assert_alternates(&t.nodes);
        assert_eq!(text(&t, &t.nodes[0]), "<ul><template");
        let each = &t.groups[0];
        assert_eq!(
            (
                each.nested,
                each.directives[0].kind,
                each.directives[0].name.as_str()
            ),
            (false, Dir::Each, "item")
        );
        assert_eq!(
            (
                each.directives[0].mods.as_slice(),
                each.directives[0].value.as_ref().unwrap().src.as_str()
            ),
            (["i".to_string()].as_slice(), "data.list")
        );
        assert_eq!(
            (t.groups[1].nested, t.groups[1].locals.as_slice()),
            (true, ["item".to_string(), "i".to_string()].as_slice())
        );
        assert_eq!(
            (t.groups[2].nested, t.groups[2].directives[0].kind),
            (true, Dir::If)
        );
        assert_eq!(t.groups[3].locals, ["item", "i"]);
        assert_eq!((t.groups[4].nested, t.groups[4].locals.len()), (false, 0));
        // `each` and `if` are plain attributes elsewhere.
        assert!(parse("<p each=\"x\" if=\"y\">").unwrap().groups.is_empty());
    }

    #[test]
    fn directive_errors() {
        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("<a on:click>").contains("needs a value"));
        assert!(err("<a on:click=\"\">").contains("needs a value"));
        assert!(err("<a on:click={go}>").contains("braces are Rust"));
        assert!(err("<a on:click=go>").contains("in quotes"));
        assert!(err("<a on:click.nope=\"go\">").contains("`.nope` is not an event modifier"));
        assert!(err("<a on:click.debounce.fast=\"go\">").contains("`.fast`"));
        assert!(err("<a on:=\"go\">").contains("event's name"));
        assert!(err("<a bind:text-x=\"x\">").contains("binds a property"));
        assert!(err("<a transition:spin-x>").contains("needs a transition"));
        assert!(err("<a use:a-b>").contains("name of a function"));
        assert!(err("<a :=\"x\">").contains("a name after"));
        assert!(err("<template each=\"x of xs\">").contains("item in list"));
        assert!(err("<template each=\"x in xs\" on:click=\"f\">").contains("no other directive"));
        assert!(err("<a {#if c}on:click=\"f\"{/if}>").contains("inside a {#…} block"));
        let e = parse("<p>\n  <a :x='1' on:click.bad=\"f\">").unwrap_err();
        assert_eq!((e.line, e.col), (2, 13));
    }

    #[test]
    fn browser_code_is_part_of_the_shape() {
        let a = parse("<p :hidden=\"a\">Hi</p><script>let a</script>").unwrap();
        let b = parse("<p :hidden=\"a\" class=\"x\">Bye</p>\n<script>let a</script>").unwrap();
        let c = parse("<p :hidden=\"!a\">Hi</p><script>let a</script>").unwrap();
        let d = parse("<p :hidden=\"a\">Hi</p><script>let b</script>").unwrap();
        assert_eq!(a.shape, b.shape);
        assert_ne!(a.shape, c.shape);
        assert_ne!(a.shape, d.shape);
    }

    #[test]
    fn inline_scripts_are_modules() {
        let t = parse(r#"<script src="/x.js"></script><script type="application/ld+json">{}</script><SCRIPT defer>go()</SCRIPT>"#).unwrap();
        assert_eq!(
            text(&t, &t.nodes[0]),
            r#"<script src="/x.js"></script><script type="application/ld+json">{}</script><SCRIPT defer type="module">go()</SCRIPT>"#
        );
    }

    #[test]
    fn pre_keeps_whitespace() {
        let t = parse("<pre>\n  a\n    b</pre>\n  <p>").unwrap();
        assert_eq!(text(&t, &t.nodes[0]), "<pre>\n  a\n    b</pre>\n<p>");
    }

    #[test]
    fn standalone_block_tags_leave_no_blank_lines() {
        let t = parse("<ul>\n  {#each xs as x}\n    <li>{x}</li>\n  {/each}\n</ul>").unwrap();
        let Node::Each { body, .. } = &t.nodes[1] else {
            panic!("{:?}", t.nodes)
        };
        assert_eq!(text(&t, &t.nodes[0]), "<ul>\n");
        assert_eq!(text(&t, &body[0]), "<li>");
        assert_eq!(text(&t, &body[2]), "</li>\n");
        assert_eq!(text(&t, &t.nodes[2]), "</ul>");

        // Not alone on its line: whitespace around the tag is content.
        let t = parse("<p>{#if a}A{/if}\n<b>B</b>").unwrap();
        assert_eq!(text(&t, &t.nodes[2]), "\n<b>B</b>");
        let t = parse("<pre>\n{#if a}\nA\n{/if}\n</pre>").unwrap();
        let Node::If { branches, .. } = &t.nodes[1] else {
            panic!()
        };
        assert_eq!(text(&t, &branches[0].1[0]), "\nA\n");
    }

    #[test]
    fn blocks() {
        let src = "{#if a}\nA\n{:else if let Some(x) = b}\n{x}\n{:else}\nC\n{/if}\
                   {#each items as (k, v), i}{k}{:else}none{/each}\
                   {#match m}\n  {:case Some(x) if x > 1}big{:case _}small{/match}";
        let t = parse(src).unwrap();
        assert_alternates(&t.nodes);
        match &t.nodes[1] {
            Node::If {
                branches,
                otherwise,
            } => {
                assert_eq!(branches.len(), 2);
                assert_eq!(branches[1].0.src, "let Some(x) = b");
                assert!(otherwise.is_some());
            }
            n => panic!("{n:?}"),
        }
        match &t.nodes[3] {
            Node::Each {
                iter,
                pat,
                index,
                otherwise,
                ..
            } => {
                assert_eq!(
                    (iter.src.as_str(), pat.as_str(), index.as_deref()),
                    ("items", "(k, v)", Some("i"))
                );
                assert!(otherwise.is_some());
            }
            n => panic!("{n:?}"),
        }
        match &t.nodes[5] {
            Node::Match { arms, .. } => assert_eq!(arms[0].0.src, "Some(x) if x > 1"),
            n => panic!("{n:?}"),
        }
    }

    #[test]
    fn each_with_cast() {
        assert_eq!(
            split_each("0..n as usize as i"),
            Some(("0..n as usize", "i", None))
        );
        assert_eq!(split_each("xs"), None);
    }

    #[test]
    fn conditional_attribute_inside_tag() {
        let t = parse("<input {#if on}checked{/if}>").unwrap();
        assert_alternates(&t.nodes);
    }

    #[test]
    fn boolean_attributes() {
        let t = parse(
            "<button class=\"k\"
  DISABLED ={!ok} name=k>",
        )
        .unwrap();
        assert_alternates(&t.nodes);
        assert_eq!(text(&t, &t.nodes[0]), "<button class=\"k\"");
        assert!(
            matches!(&t.nodes[1], Node::Bool { name, code, .. } if name == "disabled" && code.src == "!ok")
        );
        assert_eq!(text(&t, &t.nodes[2]), " name=k>");
        assert!(
            parse("<input checked=\"{on}\">")
                .unwrap_err()
                .msg
                .contains("on or off")
        );
        assert!(parse("<details open={@html x}>").is_err());
    }

    /// The nodes of `src` as a line of text: `[name=code]` for an attribute,
    /// `[+name?code]` for a class or boolean attribute.
    fn sketch(src: &str) -> String {
        let t = parse(src).unwrap();
        t.nodes
            .iter()
            .map(|n| match n {
                Node::Text(i) => t.chunks[*i].clone(),
                Node::Attr { name, code, .. } => format!("[{name}={}]", code.src),
                Node::Bool { name, code, .. } => format!("[+{name}?{}]", code.src),
                _ => "?".into(),
            })
            .collect()
    }

    /// `sketch`, with what an action's form adds spelled out.
    fn forms(src: &str) -> String {
        fn nodes(t: &Template, ns: &[Node]) -> String {
            ns.iter()
                .map(|n| match n {
                    Node::Text(i) => t.chunks[*i].clone(),
                    Node::Attr { name, code, .. } => format!("[{name}={}]", code.src),
                    Node::Bool { name, code, .. } => format!("[+{name}?{}]", code.src),
                    Node::Expr(c) => format!("{{{}}}", c.src),
                    Node::Const(c) => format!("{{@const {}}}", c.src),
                    Node::Problem { name, .. } => format!("<problem {name}>"),
                    Node::Kept {
                        name, sent, own, ..
                    } => {
                        let own = own.as_ref().map(|o| format!("|{}", nodes(t, o)));
                        format!(
                            "{{kept {name}:{}{}}}",
                            nodes(t, sent),
                            own.unwrap_or_default()
                        )
                    }
                    Node::Chosen { name, own, .. } => {
                        format!("{{chosen {name}|{}}}", own.as_deref().unwrap_or(""))
                    }
                    Node::Each { iter, body, .. } => {
                        format!("{{each {}}}{}{{/each}}", iter.src, nodes(t, body))
                    }
                    _ => "?".into(),
                })
                .collect()
        }
        let t = parse(src).unwrap();
        nodes(&t, &t.nodes)
    }

    #[test]
    fn attribute_values_may_be_left_out() {
        // `Option`s are told apart from other values by the compiler, not here.
        assert_eq!(
            sketch("<a aria-hidden={h} id=x>"),
            "<a[aria-hidden=h] id=x>"
        );
        assert!(matches!(
            &parse("<a href={u}>").unwrap().nodes[1],
            Node::Attr { url: true, .. }
        ));
        // Quoted values are text with holes in it.
        assert!(matches!(
            &parse("<a id=\"n{u}\">").unwrap().nodes[1],
            Node::Expr(_)
        ));
    }

    #[test]
    fn shorthand_attributes() {
        assert_eq!(sketch("<a {href} {id}>x"), "<a[href=href][id=id]>x");
        assert_eq!(sketch("<b {disabled}>"), "<b[+disabled?disabled]>");
        let err = parse("<a {x.y}>").unwrap_err().msg;
        assert!(err.contains("{name} for name={name}"), "{err}");
    }

    #[test]
    fn server_classes_join_the_class_attribute() {
        let want = "<p class=\"a[+on?x > 1][+b?c]\" id=\"q\">";
        assert_eq!(
            sketch("<p class=\"a\" class:on={x > 1} class:b={c} id=\"q\">"),
            want
        );
        assert_eq!(
            sketch("<p class:on={x > 1} class:b={c} class=\"a\" id=\"q\">"),
            want
        );
        assert_eq!(sketch("<p class:b={c}>"), "<p class=\"[+b?c]\">");
        assert_eq!(sketch("<i class:b={c} />"), "<i class=\"[+b?c]\"/>");
        // With other holes in the tag, and one in the class.
        assert_eq!(
            sketch("<a {href} class=\"k {m}\" class:on={c}>"),
            "<a[href=href] class=\"k ?[+on?c]\">"
        );
        // The browser's `class:` still takes quoted JavaScript.
        assert!(parse("<p class:open=\"x\">").unwrap().is_live());
    }

    #[test]
    fn action_forms_post_and_keep_what_was_typed() {
        // Each input keeps what was sent (`{kept a}`: what the action
        // refused, else its own value) and is followed by its problem.
        assert_eq!(
            forms("<form action=\"?/add\"><input name=\"a\"><input name=b /></form>"),
            "<form action=\"?/add\" method=\"post\"><input name=\"a\"{kept a:[value=__k]}><problem a>\
             <input name=b{kept b:[value=__k]}/><problem b></form>"
        );
        let t = parse("<form method=\"POST\"><input name=\"a\"></form>").unwrap();
        assert_alternates(&t.nodes);
        // Its own `value={…}` is what it shows on a GET.
        assert_eq!(
            forms("<form method=\"post\"><input name=\"t\" value={post.title}></form>"),
            "<form method=\"post\"><input name=\"t\"{kept t:[value=__k]|[value=post.title]}><problem t></form>"
        );
        // A value written as text too, decoded; one a node follows stays.
        assert_eq!(
            forms(
                "<form method=\"post\"><input value=\"a&amp;b\" name=\"t\"><input value=x name=u>\
                 <input value=\"y\" name=\"v\" title={t}></form>"
            ),
            "<form method=\"post\"><input name=\"t\"{kept t:[value=__k]|[value=\"a&b\"]}><problem t>\
             <input name=u{kept u:[value=__k]|[value=\"x\"]}><problem u>\
             <input value=\"y\" name=\"v\"[title=t]><problem v></form>"
        );
        // A textarea's content likewise, its problem after it.
        let t = parse("<form method=\"post\"><textarea name=\"b\">{post.body}</textarea></form>")
            .unwrap();
        assert_alternates(&t.nodes);
        assert_eq!(
            forms(
                "<form method=\"post\"><textarea name=\"b\">{post.body}</textarea><textarea name=c></textarea></form>"
            ),
            "<form method=\"post\"><textarea name=\"b\">{kept b:{__k}|{post.body}}</textarea><problem b>\
             <textarea name=c>{kept c:{__k}|}</textarea><problem c></form>"
        );
        // A select chooses its option by what was sent, else its value;
        // an option's value as the browser sends it: decoded, or its text.
        assert_eq!(
            forms(
                "<form method=\"post\"><select name=\"k\" value={post.kind}><option value=\"a&amp;b\">A</option>\
                   {#each ks as k}<option value={k.id}>{k.name}</option>{/each}<option>\n c  d </option><option>{e}</option></select></form>"
            ),
            format!(
                "<form method=\"post\"><select name=\"k\"{{chosen k|post.kind}}>\
                 <option value=\"a&amp;b\"[+selected?{IS}\"a&b\")]>A</option>{{each ks}}<option[value=k.id][+selected?{IS}&(k.id))]>{{k.name}}</option>{{/each}}\
                 <option[+selected?{IS}\"c d\")]>\nc  d </option><option>{{e}}</option></select><problem k></form>"
            )
        );
        // A password or a file is never sent back; its problem is shown.
        assert_eq!(
            forms(
                "<form action=\"?/a\"><input name=\"p\" type=\"password\"><input type=file name=f></form>"
            ),
            "<form action=\"?/a\" method=\"post\"><input name=\"p\" type=\"password\"><problem p>\
             <input type=file name=f><problem f></form>"
        );
        // A file that shows a field's problem itself gets none added for
        // it, and `{cx.problem("x")}` is the same element.
        assert_eq!(
            forms(
                "<form method=\"post\"><input name=\"a\"><input name=\"b\"><p>{cx.problem(\"a\")}</p></form>"
            ),
            "<form method=\"post\"><input name=\"a\"{kept a:[value=__k]}><input name=\"b\"{kept b:[value=__k]}><problem b>\
             <p><problem a></p></form>"
        );
        // Any literal, before its input or in a block; a name worked out
        // or a call in a script takes no field's away.
        assert_eq!(
            forms(
                "{#if x}<i>{cx.problem(r\"a\")}</i>{/if}<form method=\"post\"><input name=\"a\"></form>"
            ),
            "?<form method=\"post\"><input name=\"a\"{kept a:[value=__k]}></form>"
        );
        for src in [
            "<form method=\"post\"><input name=\"a\">{cx.problem(f)}</form>",
            "<form method=\"post\"><input name=\"a\"></form><script src=x>problem(\"a\")</script>",
        ] {
            assert!(forms(src).contains("<problem a>"), "{src}");
        }
        // Told otherwise (a GET), not an action, or not text:
        // left as written.
        for src in [
            "<form action=\"?/a\" method=\"get\"><input name=\"q\" value=\"x\"></form>",
            "<form action=\"/a\"><input name=\"q\"></form>",
            "<form action=\"?/a\"><input type=\"checkbox\" name=\"c\"><input type=hidden name=h></form>",
            "<form action=\"?/a\"><input name=\"q\" bind:value=\"q\"></form>",
            "<input name=\"q\">",
        ] {
            assert!(!forms(src).contains("{kept"), "{src}: {}", forms(src));
        }
        assert!(
            sketch("<form action=\"?/a\"></form><input name=\"q\">")
                .ends_with("<input name=\"q\">")
        );
        assert_eq!(
            sketch(
                "<button class=\"x\" action=\"?/rm&id={id}\" {d}>x</button><button action=\"/a\">"
            ),
            "<form method=\"post\"><button class=\"x\" formaction=\"?/rm&id=?\"[d=d]>x</button></form><button action=\"/a\">"
        );
        assert_eq!(
            sketch("<form action='?/a&b={b}'><button action=\"?/c\">x</button></form>"),
            "<form action='?/a&b=?' method=\"post\"><button action=\"?/c\">x</button></form>"
        );
        // A button that posts to another action skips the form's checks.
        assert_eq!(
            sketch(
                "<form action=\"?/a\"><button formaction=\"?/b&id=1\">b</button>\
                 <button formaction=\"?/a\">a</button><input type=submit formaction=\"?/{x}\">\
                 <button formaction=\"?/b\" formnovalidate>c</button></form>\
                 <form action=\"/x\"><button formaction=\"?/b\">d</button></form>"
            ),
            "<form action=\"?/a\" method=\"post\"><button formaction=\"?/b&id=1\" formnovalidate>b</button>\
             <button formaction=\"?/a\">a</button><input type=submit formaction=\"?/?\" formnovalidate>\
             <button formaction=\"?/b\" formnovalidate>c</button></form>\
             <form action=\"/x\"><button formaction=\"?/b\">d</button></form>"
        );
    }

    #[test]
    fn a_top_level_title_goes_in_the_head_and_slot_renders_children() {
        let t = parse("<title>{t}</title><h1>x</h1><svg><title>s</title></svg>").unwrap();
        assert_alternates(&t.nodes);
        assert!(
            matches!(&t.nodes[1], Node::Head(b) if b.len() == 3),
            "{:?}",
            t.nodes
        );
        assert!(
            t.chunks
                .iter()
                .any(|c| c.contains("<svg><title>s</title></svg>"))
        );
        let t = parse("{#if a}<title>x</title>{/if}").unwrap();
        assert!(
            !format!("{:?}", t.nodes).contains("Head"),
            "only at the top level"
        );
        let t = parse("<main><slot /></main><slot name=\"x\"></slot>").unwrap();
        assert!(t.uses_children && t.nodes.contains(&Node::Render));
        assert!(
            t.chunks
                .iter()
                .any(|c| c.contains("<slot name=\"x\"></slot>"))
        );
    }

    #[test]
    fn head_and_render() {
        let t = parse("<wisp:head><title>{t}</title></wisp:head><main>{@render children()}</main>")
            .unwrap();
        assert!(t.uses_children);
        assert_alternates(&t.nodes);
        assert!(matches!(&t.nodes[1], Node::Head(b) if b.len() == 3));
    }

    #[test]
    fn errors_have_positions() {
        let e = parse("<p>\n  {#if x}\n</p>").unwrap_err();
        assert_eq!((e.line, e.col), (2, 3));
        let e = parse("{/each}").unwrap_err();
        assert!(e.msg.contains("no block"));
        let e = parse("{#match x} junk {:case _}{/match}").unwrap_err();
        assert!(e.msg.contains("case"));
        // Broken input the fuzz tests found: errors, not panics.
        for src in [
            "{/}",
            "{#if x}{/}",
            "<title></wisp:head></title>",
            "<d\"\"\"{#if n}=\"br\"{:else}\"",
        ] {
            assert!(parse(src).is_err(), "{src}");
        }
    }

    #[test]
    fn shape_ignores_text_only() {
        let a = parse("<h1>Hello {name}</h1>").unwrap();
        let b = parse("<h2 class='big'>Goodbye {name}!</h2>").unwrap();
        let c = parse("<h1>Hello {name.len()}</h1>").unwrap();
        let d = parse("<a href={name}>Hello</a>").unwrap();
        assert_eq!(a.shape, b.shape);
        assert_ne!(a.shape, c.shape);
        assert_ne!(a.shape, d.shape); // same code, but guarded as a URL
        assert_eq!(a.chunks.len(), b.chunks.len());
    }

    /// The markup as the browser gets it, groups as `[g]`, first paints as `(g)`.
    fn markup(t: &Template, nodes: &[Node], out: &mut String) {
        for n in nodes {
            match n {
                Node::Text(i) => out.push_str(&t.chunks[*i]),
                Node::Live { group } => out.push_str(&format!("[{group}]")),
                Node::Hole { group } => out.push_str(&format!("({group})")),
                Node::Client(branches) => {
                    for (g, body) in branches {
                        out.push_str(&format!("<template[{g}]>"));
                        markup(t, body, out);
                        out.push_str("</template>");
                    }
                }
                _ => out.push('?'),
            }
        }
    }

    fn flat(src: &str) -> (String, Template) {
        let t = parse(src).unwrap();
        let mut s = String::new();
        markup(&t, &t.nodes, &mut s);
        (s, t)
    }

    #[test]
    fn live_holes_and_attributes() {
        let (s, t) = flat("<p class=\"a {:b} c\" title={:t}>Hi {:name}!</p>");
        assert_eq!(
            s,
            "<p class=\"a  c\"[0]>Hi <template[1]></template>(1)<!---->!</p>"
        );
        let d = &t.groups[0].directives;
        assert_eq!(
            (
                d[0].name.as_str(),
                d[0].value.as_ref().unwrap().src.as_str()
            ),
            ("class", "`a ${(b) ?? ''} c`")
        );
        assert_eq!(
            (
                d[1].name.as_str(),
                d[1].value.as_ref().unwrap().src.as_str()
            ),
            ("title", "t")
        );
        assert_eq!(t.groups[1].directives[0].kind, Dir::Hole);
        // `{:else}` and `{:case}` stay branches.
        assert!(parse("{#if a}x{:else}y{/if}").unwrap().groups.is_empty());
        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("<a class=\"{x} {:y}\">").contains("mixes"));
        assert!(err("<a onclick=\"{:y}\">").contains("on:click"));
        assert!(err("<a {:y}>").contains("attribute's value"));
        assert!(err("{:}").contains("empty"));
    }

    #[test]
    fn client_blocks() {
        let (s, t) = flat(
            "<ul>{:#each todos as todo, i (todo.id)}<li animate:flip>{:todo.text}</li>{:else}<li>none</li>{:/each}</ul>",
        );
        assert_eq!(
            s,
            "<ul><template[0]><li[1]><template[2]></template>(2)<!----></li></template><template[3]><li>none</li></template></ul>"
        );
        let each = &t.groups[0].directives[0];
        assert_eq!(
            (each.kind, each.name.as_str(), each.mods.clone()),
            (Dir::Each, "todo", vec!["i".to_string()])
        );
        assert_eq!(each.key.as_ref().unwrap().src, "todo.id");
        assert_eq!(t.groups[1].locals, ["todo", "i"]);
        assert!(t.groups[1].nested && !t.groups[0].nested && t.groups[3].locals.is_empty());
        assert_eq!(
            t.groups[3].directives[0].value.as_ref().unwrap().src,
            "![...(todos ?? [])].length"
        );

        let (s, t) = flat("{:#if a}A{:else if b}B{:else}C{/if}");
        assert_eq!(
            s,
            "<template[0]>A</template><template[1]>B</template><template[2]>C</template>"
        );
        let cond = |g: usize| {
            t.groups[g].directives[0]
                .value
                .as_ref()
                .unwrap()
                .src
                .clone()
        };
        assert_eq!(
            [cond(0), cond(1), cond(2)],
            ["a", "!(a) && (b)", "!(a) && !(b)"]
        );

        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("{:#if a}{:else}{:else}{:/if}").contains("not allowed"));
        assert!(err("{:#each xs as}{:/each}").contains("expected {:#each"));
        assert!(err("{:#if a}{/each}").contains("does not match"));
        assert!(err("<p {:#if a}>").contains("goes in text"));
        assert!(err("{:#while a}{:/while}").contains("unknown block"));
    }

    #[test]
    fn snippets() {
        let t = parse(
            "{#snippet row(item: &Item, i)}<li>{i} {item}</li>{/snippet}\n{@render row(x, 0)}{@render footer()}",
        )
        .unwrap();
        assert!(
            matches!(&t.nodes[1], Node::Snippet { name, params, body, .. }
                if name == "row" && params == &["item: &Item", "i"] && body.len() == 5)
        );
        assert!(
            matches!(&t.nodes[3], Node::RenderSnippet { name, args, local: true } if name == "row" && args.src == "x, 0")
        );
        assert!(
            matches!(&t.nodes[5], Node::RenderSnippet { name, args, local: false } if name == "footer" && args.src.is_empty())
        );

        // A snippet among a component's children, or named in its tag, is a prop.
        let t = parse(
            "{#snippet a()}A{/snippet}<Table {a} b={x}>{#snippet row(r)}{r}{/snippet}</Table>",
        )
        .unwrap();
        let Node::Component { props, .. } = &t.nodes[3] else {
            panic!("{:?}", t.nodes)
        };
        let snip = |n: &str, k: usize| PropValue::Snippet {
            name: n.into(),
            arity: k,
        };
        assert_eq!(props[0].value, snip("a", 0));
        assert!(matches!(props[1].value, PropValue::Expr(_)));
        assert_eq!(props[2].value, snip("row", 1));
        // It is out of scope after the component.
        let t = parse("<Tb>{#snippet row(r)}{r}{/snippet}</Tb>{@render row(1)}").unwrap();
        assert!(
            matches!(&t.nodes[3], Node::RenderSnippet { local: false, .. }),
            "{:?}",
            t.nodes
        );

        // `{:@render}`: the browser draws it, one `{:#each}` per parameter.
        let t = parse("{#snippet chip(label)}<b>{:label}</b>{/snippet}{:#each tags as tag}{:@render chip(tag)}{:/each}").unwrap();
        let each = t
            .groups
            .iter()
            .flat_map(|g| &g.directives)
            .find(|d| d.kind == Dir::Each && d.name == "label")
            .unwrap();
        assert_eq!(each.value.as_ref().unwrap().src, "[tag]");
        let hole = t
            .groups
            .iter()
            .rfind(|g| g.directives[0].kind == Dir::Hole)
            .unwrap();
        assert_eq!(hole.locals, ["tag", "label"]);
        assert!(client_renderable(&t.nodes));

        let err = |src: &str| parse(src).unwrap_err().msg;
        assert!(err("{#snippet r(a)}{@render r(a)}{/snippet}").contains("renders itself"));
        assert!(err("{#snippet r(a)}x{/snippet}{@render r(1, 2)}").contains("takes 1 argument"));
        assert!(err("{:#if a}{#snippet r()}x{/snippet}{:/if}").contains("outside client blocks"));
        assert!(err("{:@render nope(1)}").contains("no snippet `nope` above"));
        assert!(err("{#snippet r((a, b))}x{/snippet}{:@render r(1)}").contains("plain names"));
        assert!(err("{#snippet r(a)}x{/if}").contains("does not match"));
        assert!(err("{#snippet 1x()}x{/snippet}").contains("expected {#snippet"));
        assert!(err("{#snippet children()}x{/snippet}").contains("`children`"));
        assert!(err("<p {#snippet r()}{/snippet}>").contains("goes in text"));
    }

    #[test]
    fn client_components() {
        let (s, t) = flat(
            "{:#each xs as x}<Card title={:x} bind:open=\"o[x]\" on:pick=\"go\" size=\"2\" big><b>{:x}</b></Card>{:/each}<Card label={:y} />",
        );
        assert_eq!(
            s,
            "<template[0]><template[1]><b><template[2]></template>(2)<!----></b></template></template><template[3]></template>"
        );
        let c = &t.groups[1].directives[0];
        assert_eq!((c.kind, c.name.as_str()), (Dir::Comp, "Card"));
        let props: Vec<String> = c
            .props
            .iter()
            .map(|p| format!("{}={:?}", p.name, p.value))
            .collect();
        assert_eq!(props[0], "title=Live(Code { src: \"x\", line: 1 })");
        assert!(
            props[1].starts_with("open=Bind")
                && props[2].starts_with("pick=On")
                && props[3] == "size=Text(\"2\")"
                && props[4] == "big=Flag"
        );
        // The slot's content belongs to the page and sees the loop's names.
        assert_eq!(t.groups[2].locals, ["x"]);
        assert!(
            parse("{:#each xs as x}<Card n={x} />{:/each}")
                .unwrap_err()
                .msg
                .contains("browser values")
        );
    }

    /// Random templates, well formed or not, parse or fail with an error:
    /// never a panic. (A deterministic xorshift picks the pieces.)
    #[test]
    fn fuzz_never_panics() {
        const PIECES: &[&str] = &[
            "<p",
            "<div",
            "</p>",
            "</div>",
            ">",
            "/>",
            " ",
            "
",
            "=",
            "\"",
            "'",
            "{",
            "}",
            "{x}",
            "{:x}",
            "{:#if a}",
            "{:else}",
            "{:/if}",
            "{:#each xs as x (x.id)}",
            "{:/each}",
            "{:#key k}",
            "{:/key}",
            "{:#await p}",
            "{:then v}",
            "{:catch e}",
            "{:/await}",
            "{:#try}",
            "{:/try}",
            "{#if c}",
            "{/if}",
            "{#each v as x}",
            "{/each}",
            "{#snippet s(a)}",
            "{/snippet}",
            "{:@render s(1)}",
            "on:click=\"n++\"",
            "bind:value",
            "bind:group=\"g\"",
            "class:on",
            "in:fade",
            "out:spin",
            "transition:fly=\"{ y: 1 }\"",
            "client:visible",
            "client:media=\"(x)\"",
            "use:portal",
            ":hidden=\"h\"",
            "{:...rest}",
            "<wisp:window",
            "<wisp:element this={:t}",
            "</wisp:element>",
            "<Card",
            "</Card>",
            "<template each=\"x in xs\">",
            "</template>",
            "<script>",
            "</script>",
            "let n = 0",
            "<!--",
            "-->",
            "é",
            "😀",
            "{@props a: u8}",
            "{@html h}",
            "{:#if}",
            "{:/",
            "{:",
            "<",
            "</",
        ];
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        for round in 0..6000 {
            let src: String = (0..1 + round % 30)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    PIECES[x as usize % PIECES.len()]
                })
                .collect();
            if let Err(e) = parse(&src) {
                assert!(e.line >= 1 && !e.msg.is_empty(), "{src:?}");
            }
        }
    }
}
