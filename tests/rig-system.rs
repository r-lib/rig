use assert_cmd::prelude::*; // Add methods on commands
use predicates::prelude::*; // Used for writing assertions
use std::process::Command; // Run programs

#[test]
#[cfg(target_os = "macos")]
fn rig_system_make_links() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("rig")?;
    cmd.args(["system", "make-links"]);
    cmd.assert()
        .success();

    let mut cmd2 = Command::new("R-4.1");
    cmd2.args(["-q", "-s", "-e", "cat(as.character(getRversion()))"]);
    cmd2.assert()
        .success()
        .stdout(predicate::str::is_match("^4[.]1[.][0-9]$").unwrap());

    let mut cmd2 = Command::new("R-4.2");
    cmd2.args(["-q", "-s", "-e", "cat(as.character(getRversion()))"]);
    cmd2.assert()
        .success()
        .stdout(predicate::str::is_match("^4[.]2[.][0-9]$").unwrap());
    Ok(())
}
