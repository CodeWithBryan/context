use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn prints_version() {
    Command::cargo_bin("ctx")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("ctx"));
}

#[test]
fn init_creates_per_repo_dir() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("ctx")
        .unwrap()
        .args(["init", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized"));
    // ~/.ctx/repos/<hash>/ should now exist.
    // We don't assert exact path, but lance/ + refs aren't touched until first index.
}

#[test]
fn help_lists_all_subcommands() {
    Command::cargo_bin("ctx")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("index"))
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn unknown_subcommand_errors() {
    Command::cargo_bin("ctx")
        .unwrap()
        .arg("nonsense")
        .assert()
        .failure();
}
