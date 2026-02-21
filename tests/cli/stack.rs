#[test]
fn stack_without_args_prints_plain_tree_in_non_tty() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/tree"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat/tree"));
}
#[test]
fn stack_default_output_includes_pr_hyperlink_when_cached_pr_exists() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/link"])
        .assert()
        .success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 123, cached_pr_state = 'open' WHERE name = 'feat/link'",
        [],
    )
    .expect("seed pr number");

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://github.com/acme/stack-test/pull/123",
        ));
}
#[test]
fn stack_default_output_includes_pr_creation_link_when_pr_missing() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/new-pr"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://github.com/acme/stack-test/compare/main...feat/new-pr?expand=1",
        ));
}

#[test]
fn stack_up_and_down_switch_between_parent_and_child() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    stack_cmd(repo.path()).args(["down"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/parent");

    stack_cmd(repo.path()).args(["up"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/child");
}

#[test]
fn stack_top_and_bottom_switch_to_stack_extremes() {
    let repo = init_repo_without_origin();

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

    stack_cmd(repo.path()).args(["bottom"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/a");

    stack_cmd(repo.path()).args(["top"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/c");
}

#[test]
fn stack_down_from_stack_root_errors_instead_of_switching_to_base() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/root"]);

    stack_cmd(repo.path())
        .args(["down"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no parent branch in the stack"));
}

#[test]
fn stack_nav_from_base_branch_reports_base_not_in_stack() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not part of stack navigation"));
}

#[test]
fn stack_up_requires_disambiguation_when_branch_has_multiple_children() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/root", "--name", "feat/a"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/root"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/root", "--name", "feat/b"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/root"]);

    stack_cmd(repo.path())
        .args(["up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has multiple child branches"));
}

fn current_branch(repo: &std::path::Path) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}
