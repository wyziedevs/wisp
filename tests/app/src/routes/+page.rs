use crate::hooks::{Greeting, User};

struct Data {
    greeting: &'static str,
    user: Option<String>,
}

fn load(cx: &mut Cx) -> Data {
    Data { greeting: wisp::state::<Greeting>().0, user: cx.get::<User>().map(|u| u.0.clone()) }
}
