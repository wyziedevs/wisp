use wisp::prelude::*;

#[derive(Json)]
pub struct Todo {
    pub id: u32,
    pub text: String,
    pub done: bool,
}

#[derive(Json)]
pub struct Node {
    pub name: String,
    pub kids: Vec<Node>,
}

#[derive(Json)]
pub struct Data {
    pub title: String,
    pub todos: Vec<Todo>,
    pub empty: Vec<u32>,
    pub tree: Node,
    pub secret: String,
}

fn node(name: &str, kids: Vec<Node>) -> Node {
    Node {
        name: name.into(),
        kids,
    }
}

pub fn load() -> Data {
    Data {
        title: "Paint <me>".into(),
        todos: vec![
            Todo {
                id: 1,
                text: "one".into(),
                done: true,
            },
            Todo {
                id: 2,
                text: "two".into(),
                done: false,
            },
        ],
        empty: Vec::new(),
        secret: "secret".into(),
        tree: node(
            "root",
            vec![node("a", vec![node("a1", Vec::new())]), node("b", Vec::new())],
        ),
    }
}
