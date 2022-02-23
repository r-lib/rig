#[derive(Default, Debug, Clone)]
pub struct Rversion {
    pub version: Option<String>,
    pub url: Option<String>,
    pub arch: Option<String>,
}

#[derive(PartialEq, Clone)]
pub struct LinuxVersion {
    pub distro: String,
    pub version: String,
    pub url: String
}
