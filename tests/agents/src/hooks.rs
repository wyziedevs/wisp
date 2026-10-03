struct Db;

impl Db {
    async fn connect(_url: &str) -> Result<Db> {
        Ok(Db)
    }
}

async fn init() -> Result {
    wisp::provide(Db::connect(&wisp::env("DB_URL").or_status(500)?).await?);
    Ok(())
}
fn before(cx: &mut Cx) -> Result<Option<Response>> {
    if let Some(r) = cx.cors("*") { return Ok(Some(r)); }
    Ok(None)
}
