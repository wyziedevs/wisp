#[derive(Json)]
struct Todo {
    id: u32,
    text: String,
    done: bool,
}

#[derive(Json)]
struct Node {
    name: String,
    kids: Vec<Node>,
}

#[derive(Json)]
struct Data {
    title: String,
    todos: Vec<Todo>,
    empty: Vec<u32>,
    tree: Node,
    secret: String,
}

fn node(name: &str, kids: Vec<Node>) -> Node {
    Node { name: name.into(), kids }
}

fn load() -> Data {
    Data {
        title: "Paint <me>".into(),
        todos: vec![
            Todo { id: 1, text: "one".into(), done: true },
            Todo { id: 2, text: "two".into(), done: false },
        ],
        empty: Vec::new(),
        secret: "secret".into(),
        tree: node("root", vec![node("a", vec![node("a1", Vec::new())]), node("b", Vec::new())]),
    }
}
