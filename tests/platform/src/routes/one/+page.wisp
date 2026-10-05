---
// The whole route in one file: its data, an action and its endpoints.
// `../three` is the same route in three files.
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

mod server {
    fn put(text: String) -> usize {
        text.len()
    }

    fn delete(id: u64) -> u64 {
        id * 2
    }
}
---

<p>n = {n}</p>
<form action="?/add">
  <input aria-label="by" name="by">
  <button>Add</button>
</form>
