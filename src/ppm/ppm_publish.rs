//! `rig ppm pkg publish`: upload a package to a local source of a Package Manager
//! instance. Needs a token with the `sources:write` scope.

use std::error::Error;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use serde::Serialize;
use serde_json::Value;

#[cfg(target_os = "linux")]
use crate::linux::get_r_binary;
#[cfg(target_os = "macos")]
use crate::macos::get_r_binary;
#[cfg(target_os = "windows")]
use crate::windows::get_r_binary;

use crate::common::sc_get_default_or_fail;
use crate::output::OUTPUT;
use crate::ppm::api::{self, api_url};
use crate::ppm::ppm_repos::load_sources;
use crate::ppm::want_json;
use crate::repos::binaries::ppm_url;
use crate::utils::http_client;

#[derive(Debug, Serialize)]
struct Published {
    file: String,
    source: String,
    source_id: i64,
    binary: bool,
    distro: Option<String>,
    response: Value,
}

/// The id of a source, given its name or id. Needs a token.
fn source_id(source: &str) -> Result<(i64, String), Box<dyn Error>> {
    let sources = load_sources(None)?;
    let hit = sources
        .iter()
        .find(|s| s.name == source || s.id.to_string() == source)
        .ok_or_else(|| {
            format!(
                "No source '{}' in this Package Manager, see `rig ppm sources`",
                source
            )
        })?;
    Ok((hit.id, hit.name.clone()))
}

/// Build a source package from a package directory, with `R CMD build`, into
/// `outdir`.
fn build_package(dir: &Path, outdir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let rver = sc_get_default_or_fail()?;
    let r = get_r_binary(&rver)?;
    let dir = std::fs::canonicalize(dir)?;
    OUTPUT.info(&format!("Building package in {}", dir.display()));
    let status = std::process::Command::new(&r)
        .arg("CMD")
        .arg("build")
        .arg(crate::utils::r_path(&dir))
        .current_dir(outdir)
        .status()
        .map_err(|e| format!("cannot run {}: {}", r.display(), e))?;
    if !status.success() {
        bail!("R CMD build failed for {}", dir.display());
    }
    let built = std::fs::read_dir(outdir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.to_string_lossy().ends_with(".tar.gz"))
        .ok_or("R CMD build did not create a package file")?;
    Ok(built)
}

#[tokio::main]
async fn upload(
    url: &str,
    file: &Path,
    overwrite: bool,
    distro: Option<&str>,
) -> Result<Value, Box<dyn Error>> {
    let bytes = tokio::fs::read(file).await?;
    let filename = file
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package.tar.gz".to_string());
    let mut form = reqwest::multipart::Form::new().part(
        "file0",
        reqwest::multipart::Part::bytes(bytes).file_name(filename),
    );
    if overwrite {
        form = form.text("overwrite", "true");
    }
    if let Some(d) = distro {
        form = form.text("distro", d.to_string());
    }
    let req = crate::auth::api_request(&http_client(), reqwest::Method::POST, url).multipart(form);
    let (_, body) = api::send(req).await?;
    Ok(serde_json::from_slice(&body).unwrap_or(Value::Null))
}

pub fn sc_ppm_publish(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(
        args.get_one::<String>("path")
            .map(|s| s.as_str())
            .unwrap_or("."),
    );
    let source = args.get_one::<String>("source").unwrap();
    let binary = args.get_flag("binary");
    let distro = args.get_one::<String>("distro").map(|s| s.as_str());
    let overwrite = args.get_flag("overwrite");
    if binary && distro.is_none() {
        bail!("--binary needs --distro, the platform the binary is for, e.g. jammy");
    }

    let (id, name) = source_id(source)?;

    let tmp = tempfile::tempdir()?;
    let file = if path.is_dir() {
        if binary {
            bail!("--binary needs a binary package file, not a directory");
        }
        build_package(&path, tmp.path())?
    } else if path.is_file() {
        path.clone()
    } else {
        bail!("No such file or directory: {}", path.display());
    };

    let kind = if binary { "binaries" } else { "packages" };
    let url = api_url(&ppm_url(), &["sources", &id.to_string(), kind], &vec![])?;
    OUTPUT.info(&format!("Uploading {} to source {}", file.display(), name));
    let response = upload(url.as_str(), &file, overwrite, distro)?;

    let res = Published {
        file: file.display().to_string(),
        source: name,
        source_id: id,
        binary,
        distro: distro.map(|s| s.to_string()),
        response,
    };
    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&res)?);
    } else {
        let fname = Path::new(&res.file)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        OUTPUT.success(&format!("Published {} to source {}", fname, res.source));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_upload() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        rt.block_on(async {
            Mock::given(method("POST"))
                .and(path("/__api__/sources/3/binaries"))
                .and(body_string_contains(
                    "name=\"file0\"; filename=\"pkg_1.0.tar.gz\"",
                ))
                .and(body_string_contains("name=\"distro\""))
                .and(body_string_contains("jammy"))
                .and(body_string_contains("name=\"overwrite\""))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": 1})),
                )
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/__api__/sources/4/packages"))
                .and(header("content-type", "application/json"))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/__api__/sources/4/packages"))
                .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                    "code": 10, "error": "insufficient scope", "payload": null
                })))
                .mount(&server)
                .await;
        });
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pkg_1.0.tar.gz");
        std::fs::write(&file, b"tarball").unwrap();
        let url = format!("{}/__api__/sources/3/binaries", server.uri());
        // `upload` makes its own runtime, so call it from a plain thread
        let res = std::thread::spawn({
            let file = file.clone();
            move || upload(&url, &file, true, Some("jammy")).map_err(|e| e.to_string())
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(res["ok"], 1);

        let url = format!("{}/__api__/sources/4/packages", server.uri());
        let err =
            std::thread::spawn(move || upload(&url, &file, false, None).map_err(|e| e.to_string()))
                .join()
                .unwrap()
                .unwrap_err();
        assert!(err.contains("insufficient scope"));
        assert!(err.contains("rig auth status"));
    }
}
