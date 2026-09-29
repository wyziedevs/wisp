//! A resource under another: each user's items, at
//! `/users/[user]/items`. The route's `user` is the rows' `user`, so each
//! sees its own. Ids are random, and the rows stay in memory.

#[derive(Rest)]
#[rest(memory, ids = "random")]
struct Item {
    user: u64,
    name: String,
}
