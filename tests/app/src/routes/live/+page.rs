use wisp::prelude::*;

pub struct Data {
    pub title: String,
    pub items: Vec<Item>,
}

/// Read by the page's directives, so it is sent to the browser as JSON.
#[derive(Json)]
pub struct Item {
    pub name: &'static str,
    pub price: u32,
}

pub fn load() -> Data {
    Data { title: "Shop & <save>".into(), items: vec![Item { name: "tea", price: 3 }, Item { name: "cake \"big\"", price: 5 }] }
}
