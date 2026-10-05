// @feature data
#[model]
struct Item {
    id: u64,
    name: &'static str,
    price: u32,
}

pub async fn items() -> Vec<Item> {
    vec![
        Item { id: 1, name: "Tea", price: 3 },
        Item { id: 2, name: "Coffee", price: 4 },
        Item { id: 3, name: "Cake", price: 5 },
    ]
}
