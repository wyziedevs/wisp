/// `#[derive(Config)]`: read from the environment before `init`.
#[derive(Config)]
pub struct Conf {
    /// The search path, set in every environment.
    pub path: String,
    pub wisp_test_unset: Option<u16>,
}
