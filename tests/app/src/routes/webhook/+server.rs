//! A webhook: its body is signed with the `CARGO_PKG_NAME` secret, as
//! GitHub signs one (`x-hub-signature-256: sha256=<hex>`).

fn post(cx: &mut Cx) -> Result<&'static str> {
    cx.need_signature("CARGO_PKG_NAME", "x-hub-signature-256")?;
    Ok("ok")
}
