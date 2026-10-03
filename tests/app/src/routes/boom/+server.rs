/// Always fails, for the `report` hook.
fn get() -> Result<String> {
    Err(Error::new(500, "boom"))
}
