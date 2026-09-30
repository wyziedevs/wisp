// @feature api
async fn get() -> Vec<db::Item> {
    db::items().await
}
