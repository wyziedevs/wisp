// `../one` in three files: this, +page.wisp and +server.rs.
static N: Shared<u32> = Shared::new(0);

struct Data {
    n: u32,
}

fn load() -> Data {
    Data { n: *N.lock() }
}

#[action]
fn add(by: u32) {
    *N.lock() += by;
}
