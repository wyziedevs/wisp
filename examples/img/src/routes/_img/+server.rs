// /_img?src=/hero.png&w=640&q=75: a picture of static/, resized and cached.
fn get(cx: &mut Cx) -> Response {
    wisp::img::serve::<crate::App>(cx)
}
