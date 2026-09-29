use crate::hooks::User;
use wisp::prelude::*;

pub struct Data {
    pub name: String,
}

/// `before` sends visitors who are not signed in to /login, so a `User`
/// is always there.
pub fn load(cx: &mut Cx) -> Result<Data> {
    let user = cx.get::<User>().or_status(401)?;
    Ok(Data { name: user.0.clone() })
}
