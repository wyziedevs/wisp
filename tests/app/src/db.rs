#[model]
struct Memo {
    #[validate(len = 1..=100)]
    title: String,
    body: String,
}
pub static MEMOS: Table<Memo> = Table::saved();
