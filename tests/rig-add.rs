use assert_cmd::prelude::*; // Add methods on commands
use predicates::prelude::*; // Used for writing assertions
use std::process::Command; // Run programs

#[test]
fn rig_add_invalid_version() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("rig")?;
    cmd.args(["add", "foobar"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Failed to resolve R version"));

    Ok(())
}

#[test]
fn rig_add() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("rig")?;
    cmd.args(["add", "4.1"]);
    cmd.assert().success();

    let mut cmd2 = Command::cargo_bin("rig")?;
    cmd2.args(["ls", "--json"]);
    cmd2.assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"4.1"));

    let mut cmd3 = Command::cargo_bin("rig")?;
    cmd3.args(["add", "4.2"]);
    cmd3.assert().success();

    let mut cmd4 = Command::cargo_bin("rig")?;
    cmd4.args(["ls", "--json"]);
    cmd4.assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"4.2"));

    let mut cmd5 = Command::cargo_bin("rig")?;
    cmd5.args(["add", "devel"]);
    cmd5.assert().success();

    let mut cmd6 = Command::cargo_bin("rig")?;
    cmd6.args(["ls", "--json"]);
    cmd6.assert()
        .success()
        .stdout(predicate::str::contains("\"aliases\": [\"devel\"],"));

    Ok(())
}
