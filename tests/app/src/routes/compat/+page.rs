//! The long form: docs, inner attributes, explicit imports, `pub` and `Error::new`.
#![allow(dead_code)]

use wisp::prelude::*;
use wisp::Error;

pub struct Data {
    pub who: String,
}

pub async fn load(cx: &mut Cx) -> Result<Data> {
    match cx.query("who") {
        Some(who) => Ok(Data { who: who.to_string() }),
        None => Err(Error::new(400, "Who?")),
    }
}
