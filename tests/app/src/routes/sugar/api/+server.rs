use wisp::prelude::*;

pub fn get() -> Response {
    Response::json_of(&vec![("a<b", Some(1)), ("c", None)])
}
