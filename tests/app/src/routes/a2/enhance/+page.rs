use std::sync::Mutex;
use wisp::prelude::*;

static ITEMS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub struct Data {
    pub items: Vec<String>,
}

pub fn load() -> Data {
    Data {
        items: ITEMS.lock().unwrap().clone(),
    }
}

#[action]
pub async fn add(cx: &mut Cx) -> Result<()> {
    let text = cx.form().required("text")?.into_owned();
    wisp::sleep(std::time::Duration::from_millis(300)).await;
    ITEMS.lock().unwrap().push(text);
    Ok(())
}

#[action]
pub fn answer() -> Response {
    Response::json("{\"n\":42}")
}
