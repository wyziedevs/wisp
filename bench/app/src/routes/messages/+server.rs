// `/json`'s object as a `#[derive(Rest)]` row: `GET /messages/1` is
// `{"id":1,"message":"Hello, World!"}` with its ETag, read under the
// table's lock.

#[derive(Rest)]
#[rest(memory)]
struct Message {
    message: String,
}

static SEEDED: std::sync::Once = std::sync::Once::new();

fn before() {
    SEEDED.call_once(|| {
        Message::table().add(Message {
            message: "Hello, World!".into(),
        });
    });
}
