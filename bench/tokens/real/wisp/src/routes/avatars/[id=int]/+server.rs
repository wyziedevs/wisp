// @feature upload
fn get(id: u64) -> Option<Image> {
    USERS.get(id)?.value.avatar
}
