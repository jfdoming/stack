#[test]
fn untrack_command_splices_children_and_removes_branch_record() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/a", "--name", "feat/b"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/b", "--name", "feat/c"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["untrack", "feat/b", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"action\": \"untrack\""));

    let output = stack_cmd(repo.path())
        .args(["--porcelain"])
        .output()
        .expect("run stack --porcelain");
    assert!(output.status.success());
    let branches: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let rows = branches.as_array().expect("branch array");

    assert!(
        !rows.iter().any(|row| row["name"] == "feat/b"),
        "feat/b should be untracked"
    );

    let feat_c = rows
        .iter()
        .find(|row| row["name"] == "feat/c")
        .expect("feat/c entry");
    assert_eq!(feat_c["parent"], "feat/a");
}
#[test]
fn untrack_without_branch_in_non_interactive_mode_assumes_only_viable_branch() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["--yes", "untrack"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "assuming target branch 'feat/a' (only viable branch)",
        ));
}
#[test]
fn untrack_without_branch_in_non_interactive_mode_requires_argument_when_multiple_tracked() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/b"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["untrack"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "branch required in non-interactive mode",
        ));
}
#[test]
fn unlink_subcommand_is_removed() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["unlink", "feat/a"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand 'unlink'"));
}
#[test]
fn untrack_without_branch_in_non_interactive_mode_requires_yes_before_actioning_assumed_target() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["untrack"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "target branch was auto-selected as 'feat/a'",
        ));
}
#[test]
fn untrack_main_is_allowed_as_noop() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["untrack", "main", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"action\": \"untrack\""))
        .stdout(predicate::str::contains("\"status\": \"noop\""))
        .stdout(predicate::str::contains(
            "\"reason\": \"base branch cannot be untracked\"",
        ));
}
#[test]
fn untrack_without_branch_is_noop_when_only_base_exists() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["untrack", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"branch\": \"main\""))
        .stdout(predicate::str::contains("\"status\": \"noop\""));
}
