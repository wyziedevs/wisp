struct Data {
    title: String,
    items: Vec<Item>,
}

/// Read by the page's directives, so it is sent to the browser as JSON.
#[derive(Json)]
struct Item {
    name: &'static str,
    price: u32,
}

fn load() -> Data {
    Data { title: "Shop & <save>".into(), items: vec![Item { name: "tea", price: 3 }, Item { name: "cake \"big\"", price: 5 }] }
}
