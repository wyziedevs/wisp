// @feature upload
fn get(id: u64) -> Option<Image> {
    db::USERS.get(id)?.value.avatar
}
