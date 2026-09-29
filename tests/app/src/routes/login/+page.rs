use wisp::prelude::*;

#[action]
pub fn default(cx: &mut Cx) -> Result<()> {
    let name = cx.form().required("name")?.into_owned();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(error(400, "A name is letters and digits"));
    }
    cx.set_signed_cookie("user", name);
    Err(redirect("/admin"))
}

#[action]
pub fn logout(cx: &mut Cx) -> Result<()> {
    cx.set_signed_cookie("user", "");
    Err(redirect("/"))
}
