//! The app's routes as the generated code serves them, read once from the
//! route files: each route with its layouts (outermost first), its error
//! page and that page's layouts, the modules it calls, its `BODY_LIMIT`
//! and `CACHE`, and whether a request through it may wait. Codegen prints
//! from it and works out none of it again.

use crate::openapi::Op;
use crate::rust_scan::{FnItem, TypeItem};

/// Route `i` of `routes::Tree` is `routes[i]`.
#[derive(Default)]
pub struct Model {
    pub routes: Vec<Route>,
    pub layouts: Vec<Layout>,
    pub errors: Vec<ErrorPage>,
    /// The `+error.wisp` of `src/routes`, which answers a path no route
    /// matches.
    pub root_error: Option<usize>,
}

/// A `+layout.wisp`: layout `l`'s module is `layout_{l}`, its data `d{l}`.
pub struct Layout {
    /// Its template (an index into codegen's templates).
    pub tpl: usize,
    /// Its Rust has `load`.
    pub load: bool,
    /// Its Rust has an `async fn`.
    pub waits: bool,
}

/// A `+error.wisp`, rendered inside the layouts of its own directory.
pub struct ErrorPage {
    pub tpl: usize,
    /// Indices into `Model::layouts`, outermost first.
    pub layouts: Vec<usize>,
}

pub struct Route {
    /// `/blog/[slug]`, for comments and messages.
    pub pattern: String,
    pub page: Option<Page>,
    pub server: Option<Server>,
    /// Indices into `Model::layouts`, outermost first.
    pub layouts: Vec<usize>,
    /// Index into `Model::errors` of the nearest `+error.wisp`.
    pub error: Option<usize>,
    /// The module whose `BODY_LIMIT` applies.
    pub body_limit: Option<String>,
    /// The page module whose actions take uploads (`__call::UPLOADS`
    /// bytes), which raise the body limit.
    pub uploads: Option<String>,
    pub cache: Option<Cache>,
    /// `/sitemap.xml` lists it: a page outside any `(private)` group,
    /// without a `<meta name="robots" content="noindex">`.
    pub indexed: bool,
}

impl Route {
    /// What a `CACHE` keeps of a GET is kept apart per `accept`: its GET
    /// varies by it (a `#[derive(Rest)]` list). Only such a route pays for
    /// the key.
    pub fn by_accept(&self) -> bool {
        let handlers = self.server.iter().flat_map(|s| &s.handlers);
        handlers
            .filter(|h| h.op.method == "get")
            .any(|h| h.by_accept)
    }
}

/// A route's `+page.wisp`.
pub struct Page {
    /// The module of its Rust, `page_{i}`.
    pub module: String,
    pub tpl: usize,
    /// The functions of its Rust: `load`, actions, `entries`, helpers.
    pub fns: Vec<FnItem>,
    /// Its statements may wait, or its Rust has an `async fn`.
    pub waits: bool,
}

impl Page {
    pub fn load(&self) -> bool {
        self.fns.iter().any(|f| f.name == "load")
    }

    pub fn entries(&self) -> bool {
        self.fns.iter().any(|f| f.name == "entries")
    }

    pub fn actions(&self) -> impl Iterator<Item = &FnItem> {
        self.fns.iter().filter(|f| f.action)
    }
}

/// The `+server.rs` a route is served by: the route's own handlers, or
/// its `/[id]`'s.
pub struct Server {
    pub module: String,
    pub handlers: Vec<Handler>,
    /// It has `fn before`, which runs before each of its handlers.
    pub before: bool,
    /// The types it defines, for the OpenAPI document: one copy, which
    /// each route it serves shares.
    pub types: std::rc::Rc<[TypeItem]>,
    /// It has an `async fn`.
    pub waits: bool,
}

/// A handler of a `+server.rs`: its shim in `__call`, whether it is for
/// the `/[id]`, and the operation it is (the method it answers, what it
/// takes and returns) for the OpenAPI document.
#[derive(Clone)]
pub struct Handler {
    pub shim: String,
    pub member: bool,
    pub op: Op,
    /// Its answer depends on the request's `accept` (a `#[derive(Rest)]`
    /// list is JSON or NDJSON), besides its path and query.
    pub by_accept: bool,
    /// Its shim has a sync twin, `{shim}_now`.
    pub sync: bool,
    /// It returns nothing, and its shim `()`: the answer is a 204.
    pub empty: bool,
}

/// A route's `CACHE` (or `CACHE_PUBLIC`).
pub struct Cache {
    /// The module that sets it.
    pub module: String,
    pub public: bool,
}

impl Model {
    /// Whether a request through `r` may wait on something: its page's
    /// statements, or an `async fn` of a module it may call (its page's,
    /// its layouts', its `+server.rs`, its error page's layouts'). Every
    /// doubt counts as waiting (see `Gen::routes`).
    pub fn route_waits(&self, r: &Route) -> bool {
        r.page.as_ref().is_some_and(|p| p.waits)
            || r.server.as_ref().is_some_and(|s| s.waits)
            || self.layouts_wait(&r.layouts)
            || r.error.is_some_and(|e| self.error_waits(e))
    }

    /// Whether one of `layouts` may wait.
    fn layouts_wait(&self, layouts: &[usize]) -> bool {
        layouts.iter().any(|&l| self.layouts[l].waits)
    }

    /// Whether error page `e` may wait: one of its layouts may.
    pub fn error_waits(&self, e: usize) -> bool {
        self.layouts_wait(&self.errors[e].layouts)
    }

    /// Whether the root error page, which answers unmatched paths, may wait.
    pub fn root_waits(&self) -> bool {
        self.root_error.is_some_and(|e| self.error_waits(e))
    }
}
