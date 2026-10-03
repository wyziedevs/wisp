// @feature api
async fn get() -> Vec<Item> {
    items().await
}
