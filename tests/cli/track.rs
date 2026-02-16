#[cfg(unix)]
#[test]
fn track_infer_uses_fork_qualified_head_for_pr_detection() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/fork-pr"]);
    run_git(repo.path(), &["checkout", "main"]);

    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:alice/stack-test.git",
        ],
    );
    run_git(
        repo.path(),
        &["config", "branch.feat/fork-pr.remote", "origin"],
    );

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"--head feat/fork-pr\"* ]]; then\n  echo '[]'\n  exit 0\nfi\nif [[ \"$*\" == *\"--head alice:feat/fork-pr\"* ]]; then\n  echo '[{\"number\": 42, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"mergeCommit\": null}]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["track", "feat/fork-pr", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack track infer dry-run");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["changes"][0]["branch"], "feat/fork-pr");
    assert_eq!(json["changes"][0]["new_parent"], "main");
    assert_eq!(json["changes"][0]["source"], "pr_base");
}
#[test]
fn track_command_sets_parent_for_existing_branch() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["track", "feat/a", "--parent", "main", "--porcelain"])
        .output()
        .expect("run stack track");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["mode"], "single");
    assert_eq!(json["applied"], true);
    assert_eq!(json["changes"][0]["branch"], "feat/a");
    assert_eq!(json["changes"][0]["new_parent"], "main");
    assert_eq!(json["changes"][0]["source"], "explicit");
}
#[test]
fn track_without_branch_in_non_interactive_mode_assumes_only_viable_branch() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--parent", "main", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "assuming target branch 'feat/a' (only viable branch)",
        ));
}
#[test]
fn track_without_branch_in_non_interactive_mode_requires_yes_before_actioning_assumed_target() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--parent", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "target branch was auto-selected as 'feat/a'",
        ));

    stack_cmd(repo.path())
        .args(["--yes", "track", "--parent", "main"])
        .assert()
        .success();
}
#[test]
fn track_without_branch_in_non_interactive_mode_requires_argument_when_multiple_branches() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--parent", "main", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "branch required in non-interactive mode",
        ));
}
#[test]
fn track_without_parent_in_non_interactive_mode_infers_parent_by_default() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "feat/a", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "would track 'feat/a' under 'main'",
        ));
}
#[test]
fn track_without_parent_in_non_interactive_mode_uses_inference_when_possible() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "feat/b", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "would track 'feat/b' under 'feat/a'",
        ));
}

#[test]
fn track_inference_recurses_to_base_branch_when_reachable() {
    let repo = init_repo();

    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);

    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    fs::write(repo.path().join("b.txt"), "b\n").expect("write b");
    run_git(repo.path(), &["add", "b.txt"]);
    run_git(repo.path(), &["commit", "-m", "b"]);

    run_git(repo.path(), &["checkout", "-b", "feat/c"]);
    fs::write(repo.path().join("c.txt"), "c\n").expect("write c");
    run_git(repo.path(), &["add", "c.txt"]);
    run_git(repo.path(), &["commit", "-m", "c"]);
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["track", "feat/c", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack track infer dry-run");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let changes = json["changes"].as_array().expect("changes array");
    let has_c_to_b = changes
        .iter()
        .any(|c| c["branch"] == "feat/c" && c["new_parent"] == "feat/b");
    let has_b_to_a = changes
        .iter()
        .any(|c| c["branch"] == "feat/b" && c["new_parent"] == "feat/a");
    let has_a_to_main = changes
        .iter()
        .any(|c| c["branch"] == "feat/a" && c["new_parent"] == "main");
    assert!(has_c_to_b, "expected feat/c -> feat/b, got: {json}");
    assert!(has_b_to_a, "expected feat/b -> feat/a, got: {json}");
    assert!(has_a_to_main, "expected feat/a -> main, got: {json}");
}
#[test]
fn track_infer_dry_run_reports_inferred_parent() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    fs::write(repo.path().join("b.txt"), "b\n").expect("write b");
    run_git(repo.path(), &["add", "b.txt"]);
    run_git(repo.path(), &["commit", "-m", "b"]);
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["track", "feat/b", "--infer", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack track infer dry-run");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["applied"], false);
    assert_eq!(json["changes"][0]["branch"], "feat/b");
    assert_eq!(json["changes"][0]["new_parent"], "feat/a");
}
#[test]
fn track_conflict_in_non_interactive_mode_requires_force() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/c"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "feat/c", "--parent", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["track", "feat/c", "--parent", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("use --force"));

    stack_cmd(repo.path())
        .args(["track", "feat/c", "--parent", "main", "--force"])
        .assert()
        .success();
}
#[test]
fn track_all_non_interactive_errors_when_unresolved() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--all"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "some branches could not be resolved",
        ));
}
