#[test]
fn completions_command_generates_script() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_stack"));
}
#[test]
fn completions_without_shell_in_non_interactive_mode_requires_argument() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["completions"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "shell required in non-interactive mode",
        ));
}
