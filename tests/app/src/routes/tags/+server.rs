//! Handlers that take an `id` the route does not have serve `/tags/[id]`;
//! `list` is the GET of `/tags`.

static TAGS: Table<String> = Table::new();

/// Runs before each handler here.
fn before(cx: &mut Cx) {
    cx.set_header("x-tags", "yes");
}

fn list() -> Vec<Row<String>> {
    TAGS.all()
}

fn post(name: String) -> u64 {
    TAGS.add(name)
}

fn get(id: u64) -> Option<Row<String>> {
    TAGS.get(id)
}

fn delete(id: u64) -> Option<()> {
    TAGS.remove(id).map(drop)
}
