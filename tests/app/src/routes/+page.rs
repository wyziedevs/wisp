use crate::hooks::{Greeting, User};
use wisp::prelude::*;

pub struct Data {
    pub greeting: &'static str,
    pub user: Option<String>,
}

pub fn load(cx: &mut Cx) -> Data {
    Data { greeting: wisp::state::<Greeting>().0, user: cx.get::<User>().map(|u| u.0.clone()) }
}
