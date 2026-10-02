/// A member's picture, as itself: its type, an ETag, 304 when unchanged.
fn get(id: u64) -> Option<Image> {
    people::PEOPLE.get(id)?.value.avatar
}
