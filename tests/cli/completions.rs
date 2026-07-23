#[test]
fn completions_command_generates_script() {
    let dir = tempfile::tempdir().expect("tempdir");
    stack_cmd(dir.path())
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_stack"));
}
#[test]
fn completions_without_shell_in_non_interactive_mode_requires_argument() {
    let dir = tempfile::tempdir().expect("tempdir");
    stack_cmd(dir.path())
        .args(["completions"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "shell required in non-interactive mode",
        ));
}
