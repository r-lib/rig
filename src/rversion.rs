#[derive(Default, Debug, Clone)]
pub struct Rversion {
    pub version: String,
    pub url: Option<String>,
    pub arch: String,
}
