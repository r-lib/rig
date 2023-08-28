
use regex::Regex;
use std::env;
use std::error::Error;
use std::path::Path;
use std::path::PathBuf;

use clap::ArgMatches;
use semver::Version;
use simple_error::*;
use simplelog::*;
use tabular::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::download::download_json_sync;
use crate::escalate::escalate;
use crate::renv;
use crate::rversion::*;
use crate::run::*;
use crate::utils::*;

pub fn check_installed(x: &String) -> Result<String, Box<dyn Error>> {
    let inst = sc_get_list_details()?;

    for ver in inst {
        if &ver.name == x {
            return Ok(ver.name);
        }
        if ver.aliases.contains(x) {
            debug!("Alias {} is resolved to version {}", x, ver.name);
            return Ok(ver.name);
        }
    }

    bail!("R version <b>{}</b> is not installed", &x);
}

// -- rig default ---------------------------------------------------------

// Fail if no default is set

pub fn sc_get_default_or_fail() -> Result<String, Box<dyn Error>> {
    let default = sc_get_default()?;
    match default {
        None => bail!("No default R version is set, call <b>rig default <version></b>"),
        Some(d) => Ok(d),
    }
}

pub fn set_default_if_none(ver: String) -> Result<(), Box<dyn Error>> {
    let cur = sc_get_default()?;
    if cur.is_none() {
        sc_set_default(&ver)?;
    }
    Ok(())
}

// -- rig list ------------------------------------------------------------

pub fn sc_get_list_details() -> Result<Vec<InstalledVersion>, Box<dyn Error>> {
    let names = sc_get_list()?;
    let aliases = find_aliases()?;
    let mut res: Vec<InstalledVersion> = vec![];
    let re = Regex::new("^Version:[ ]?")?;

    for name in names {
        let desc = Path::new(R_ROOT)
            .join(R_SYSLIBPATH.replace("{}", &name))
            .join("base/DESCRIPTION");
        let lines = match read_lines(&desc) {
            Ok(x) => x,
            Err(_) => vec![],
        };
        let idx = grep_lines(&re, &lines);
        let version: Option<String> = if idx.len() == 0 {
            None
        } else {
            Some(re.replace(&lines[idx[0]], "").to_string())
        };
        let path = Path::new(R_ROOT).join(R_VERSIONDIR.replace("{}", &name));
        let binary = Path::new(R_ROOT).join(R_BINPATH.replace("{}", &name));
        let mut myaliases: Vec<String> = vec![];
        for a in &aliases {
            if a.version == name {
                myaliases.push(a.alias.to_owned());
            }
        }
        res.push(InstalledVersion {
            name: name.to_string(),
            version: version,
            path: path.to_str().and_then(|x| Some(x.to_string())),
            binary: binary.to_str().and_then(|x| Some(x.to_string())),
            aliases: myaliases
        });
    }

    Ok(res)
}

// -- rig system add-pak (implementation) ---------------------------------

// TODO: we should not hardcode this here...
pub fn check_has_pak(ver: &String) -> Result<bool, Box<dyn Error>> {
    // cur off -arm64 and -x86_64
    let mut ver = Regex::new("-.*$")?.replace(ver, "").to_string();

    // add .0 for macOS minor versions
    let minor = Regex::new("^[0-9]+[.][0-9]+$")?;
    if minor.is_match(&ver) {
        ver = ver + ".0";
    }

    // cut off extra stuff on Windows
    ver = Regex::new("[a-zA-Z][a-zA-Z0-9]*$")?.replace(&ver, "").to_string();

    let vv = match Version::parse(&ver) {
        Ok(x) => x,
        Err(_) => return Ok(true)  // devel or next, probably
    };

    let v350 = Version::parse("3.5.0")?;
    if vv < v350 {
        bail!("Pak is only available for R 3.5.0 or later");
    }
    Ok(true)
}

pub fn system_add_pak(
    vers: Option<Vec<String>>,
    stream: &str,
    update: bool,
) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => vec![sc_get_default_or_fail()?],
    };

    for ver in vers {
        let ver = check_installed(&ver)?;
        if update {
            info!("Installing pak for R {}", ver);
        } else {
            info!("Installing pak for R {} (if not installed yet)", ver);
        }
        check_has_pak(&ver)?;

        // We do this to create the user library, because currently there
        // is a bug in the system profile code that creates it, and it is
        // only added after a restart.
        match r(&ver, "invisible()") {
            Ok(_) => {},
            Err(x) => bail!("Failed to to install pak for R {}: {}", ver, x.to_string())
        };

        // The actual pak installation
        let cmd;
        if update {
            cmd = r#"
                install.packages('pak', repos = sprintf('https://r-lib.github.io/p/pak/{}/%s/%s/%s', .Platform$pkgType, R.Version()$os, R.Version()$arch))
            "#;
        } else {
            cmd = r#"
                if (!requireNamespace('pak', quietly = TRUE)) {
                    install.packages('pak', repos = sprintf('https://r-lib.github.io/p/pak/{}/%s/%s/%s', .Platform$pkgType, R.Version()$os, R.Version()$arch))
                }
            "#;
        };
        let cmd = cmd.replace("{}", stream);

        match r(&ver, &cmd) {
            Ok(_) => {},
            Err(x) => bail!("Failed to install pak for R {}: {}", ver, x.to_string())
        };
    }

    Ok(())
}

// -- rig rstudio ---------------------------------------------------------

pub fn sc_rstudio(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let mut ver: Option<&String> = args.get_one("version");
    let mut prj: Option<&String> = args.get_one("project-file");

    #[cfg(target_os = "windows")]
    if args.get_flag("config-path") {
	let cp = get_rstudio_config_path();
	match cp {
	    Ok(x)  => println!("{}", x.display()),
	    Err(x) => bail!("Error: {}", x.to_string())
	};
	return Ok(());
    }

    if let Some(ver2) = ver {
        if ver2.ends_with("renv.lock") {
            let lockfile = PathBuf::new().join(ver2);
            let needver = renv::parse_r_version(lockfile)?;
            let usever = renv::match_r_version(&needver)?;
            let realver = usever.version.to_string();

	    // On windows we need to escalate to change the registry
	    // If we don't escalate now, then the info!() will be
	    // printed twive.
	    if std::env::consts::OS == "windows" {
		escalate("updating default version in registry")?;
	    }

            info!("Using {} R {}{}",
                  if needver == realver {
                      "matching version:"
                  } else {
                      "latest minor version:"
                  },
                  usever.name,
                  if realver != usever.name {
                      " (R ".to_string() + &realver + ")"
                  } else {
                      "".to_string()
                  }
            );

            // Seems that it is enough to call with the directory
            // of the lock file, this works reasonable with and without
            // a project file.
            let prdir = Path::new(ver2).parent();
            let prdir = prdir.and_then(|x| Some(x.as_os_str()));
            return sc_rstudio_(Some(&usever.name), None, prdir);
        }
    }

    // If the first argument is an R project file, and the second is not,
    // then we switch the two
    if let Some(ver2) = ver {
        if ver2.ends_with(".Rproj") {
            ver = args.get_one("project-file");
            prj = args.get_one("version");
        }
    }

    let ver = match ver {
        None => None,
        Some(str) => Some(&str[..])
    };

    let prj = match prj {
        None => None,
        Some(prj) => Some(&prj[..])
    };

    sc_rstudio_(ver, prj, None)
}

// -- rig avilable --------------------------------------------------------

pub fn get_platform(args: &ArgMatches)
                    -> Result<String, Box<dyn Error>> {

    // rig add does not have a --platform argument, only auto-detect
    match args.try_contains_id("platform") {
        Ok(_) => {
            let platform = args.get_one::<String>("platform");
            if let Some(x) = platform {
                return Ok(x.to_string())
            }
        },
        Err(_) => { }
    };

    #[allow(unused_mut)]
    let mut os = env::consts::OS.to_string();

    #[cfg(target_os = "linux")]
    {
        if os == "linux" {
            let dist = detect_linux()?;
            os = "linux-".to_string() + &dist.distro + "-" + &dist.version;
        }
    }

    debug!("Auto-detected platform: {}.", os);

    Ok(os)
}

pub fn get_arch(platform: &str, args: &ArgMatches) -> String {
    #[allow(unused_mut)]

    // For rig add we don't have --arch, except on macOS, only auto-detect
    let arch = match args.try_contains_id("arch") {
        Ok(_) => {
            args.get_one::<String>("arch")
        },
        Err(_) => None
    };

    // For Windows, the default is x86_64
    let arch = match arch {
        Some(x) => {
            match args.value_source("arch") {
                Some(y) => {
                    if y == clap::parser::ValueSource::DefaultValue &&
                        platform == "windows"{
                            "x86_64".to_string()
                        } else {
                            x.to_string()
                        }
                },
                None => x.to_string()
            }
        },
        None    => {
            if platform == "windows" {
                "x86_64".to_string()
            } else {
                env::consts::ARCH.to_string()
            }
        }
    };

    // Prefer 'arm64' on macos, but 'aarch64' on linux
    if platform == "macos" && arch == "aarch64" {
        "arm64".to_string()
    } else if platform == "linux" && arch == "arm64" {
        "aarch64".to_string()
    } else {
        arch
    }
}

pub fn sc_available(args: &ArgMatches, mainargs: &ArgMatches)
                    -> Result<(), Box<dyn Error>> {
    #[allow(unused_mut)]

    if args.get_flag("list-distros") {
        return sc_available_distros(args, mainargs);
    }

    let platform = get_platform(args)?;
    let arch = get_arch(&platform, args);

    let url = "https://api.r-hub.io/rversions/available/".to_string() +
        &platform + "/" + &arch;
    let resp = download_json_sync(vec![url])?;
    let resp = resp[0].as_array().unwrap();

    let mut vers: Vec<Available> = vec![];
    for item in resp.iter().rev() {
        let date = unquote(&item["date"].to_string());
        let rtype = unquote(&item["type"].to_string());
        let new = Available {
            name: unquote(&item["name"].to_string()),
            version: unquote(&item["version"].to_string()),
            date: if date == "null" { None } else { Some(date) },
            url: Some(unquote(&item["url"].to_string())),
            rtype: Some(rtype),
        };

        if ! args.get_flag("all") &&
            vers.len() > 0 &&
            new.name != "next" && new.name != "devel" {
                let lstnam = &vers[vers.len() - 1].name;
                let v300 = Version::parse("3.0.0")?;
                let lstver = Version::parse(&vers[vers.len() - 1].version)?;
                let thsver = Version::parse(&new.version)?;
                // drop old versions
                if thsver < v300 { continue; }
                // drop outdated minor versions
                if lstver.major == thsver.major &&
                    lstver.minor == thsver.minor &&
                    lstnam != "next" && lstnam != "devel" {
                        continue;
                    }
            }
        vers.push(new);
    }

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("[");
        let num = vers.len();
        for (idx, ver) in vers.iter().rev().enumerate() {
            let date = match &ver.date {
                None => "null".to_string(),
                Some(d) => "\"".to_string() + d + "\""
            };
            let rtype = match &ver.rtype {
                None => "null".to_string(),
                Some(x) => "\"".to_string() + x + "\""
            };
            let url = match &ver.url {
                None => "null".to_string(),
                Some(x) => "\"".to_string() + x + "\""
            };
            println!("  {{");
            println!("    \"name\": \"{}\",", ver.name);
            println!("    \"date\": {},", date);
            println!("    \"version\": \"{}\",", ver.version);
            println!("    \"type\": {},", rtype);
            println!("    \"url\": {}", url);
            println!("  }}{}", if idx == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "version", "release date", "type"]);
        tab.add_heading("------------------------------------------");
        for item in vers.iter().rev() {
            let date = match &item.date {
                None => "".to_string(),
                Some(d) => d[..10].to_string()
            };
            let rtype = match &item.rtype {
                None => "".to_string(),
                Some(x) => x.to_string()
            };
            tab.add_row(row!(&item.name, &item.version, date, rtype));
        }
        print!("{}", tab);
    }
    Ok(())
}

fn sc_available_distros(args: &ArgMatches, mainargs: &ArgMatches)
                        -> Result<(), Box<dyn Error>> {

    let url = "https://api.r-hub.io/rversions/linux-distros".to_string();
    let resp = download_json_sync(vec![url])?;
    let resp = resp[0].as_array().unwrap();

    if args.get_flag("json") || mainargs.get_flag("json") {
        let num = resp.len();
        println!("[");
        for (idx, item) in resp.iter().enumerate() {
            println!("{{");
            println!("  \"name\": {},", &item["name"]);
            println!("  \"version\": {},", &item["version"]);
            println!("  \"id\": {}", &item["id"]);
            println!("}}{}", if idx == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}");
        tab.add_row(row!["name", "version", "id"]);
        tab.add_heading("--------------------------------------------------------------");
        for item in resp.iter() {
            tab.add_row(row!(
                unquote(&item["name"].to_string()),
                unquote(&item["version"].to_string()),
                unquote(&item["id"].to_string())
            ));
        }

        print!("{}", tab);
    }

    Ok(())
}

// ------------------------------------------------------------------------
