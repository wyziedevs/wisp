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
fn before(cx: &mut Cx) -> Result {
    cx.cors("*")?;                    // a preflight is the Err that `?` returns
    Ok(())
}
fn after(cx: &mut Cx, reply: &mut Reply) {}   // sync, every reply: headers, logs
fn report(cx: &mut Cx, err: &Error) {}        // sync, every 5xx: Sentry and the like
