/// Where to start.
#[derive(Json)]
struct Index {
    notes: &'static str,
    events: &'static str,
    docs: &'static str,
}

fn get() -> Index {
    Index {
        notes: "/api/notes",
        events: "/api/events",
        docs: "/_wisp/docs",
    }
}
