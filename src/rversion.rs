
#[derive(Default, Debug, Clone)]
pub struct Rversion {
    pub version: Option<String>,
    pub url: Option<String>,
    pub arch: Option<String>,
}

#[derive(Default, Debug, Clone)]
pub struct InstalledVersion {
    pub name: String,
    pub version: Option<String>
}

#[derive(PartialEq, Clone, Debug)]
pub struct LinuxVersion {
    pub distro: String,
    pub version: String,
    pub url: String,
    pub rspm: bool,
    pub rspm_url: String
}

#[derive(PartialEq, Clone, Debug)]
pub struct PkgLibrary {
    pub rversion: String,
    pub name: String,
    pub path: std::path::PathBuf,
    pub default: bool
}
