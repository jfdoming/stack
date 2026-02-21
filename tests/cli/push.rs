#[test]
fn push_force_with_lease_updates_non_fast_forward_branches() {
    let repo = init_repo();
    let bare = configure_local_push_url(repo.path());

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/a"]);
    std::fs::write(repo.path().join("a.txt"), "first\n").expect("write a first");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "feat/a first"]);

    run_git(repo.path(), &["push", "--set-upstream", "origin", "feat/a"]);

    let old_remote_sha = {
        let output = Command::new("git")
            .current_dir(&bare)
            .args(["rev-parse", "refs/heads/feat/a"])
            .output()
            .expect("read old remote sha");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8 old remote sha")
            .trim()
            .to_string()
    };

    run_git(repo.path(), &["reset", "--hard", "main"]);
    std::fs::write(repo.path().join("a.txt"), "rewritten\n").expect("write rewritten a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "feat/a rewritten"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["push"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/a' to 'origin'"));

    let new_remote_sha = {
        let output = Command::new("git")
            .current_dir(&bare)
            .args(["rev-parse", "refs/heads/feat/a"])
            .output()
            .expect("read new remote sha");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8 new remote sha")
            .trim()
            .to_string()
    };

    assert_ne!(
        new_remote_sha, old_remote_sha,
        "expected force-with-lease push to rewrite feat/a on remote"
    );
}

#[test]
fn push_pushes_all_tracked_non_base_branches() {
    let repo = init_repo();
    let bare = configure_local_push_url(repo.path());

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/a", "--name", "feat/b"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/a"]);
    std::fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);

    run_git(repo.path(), &["checkout", "feat/b"]);
    std::fs::write(repo.path().join("b.txt"), "b\n").expect("write b");
    run_git(repo.path(), &["add", "b.txt"]);
    run_git(repo.path(), &["commit", "-m", "b"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["push"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/a' to 'origin'"))
        .stdout(predicate::str::contains("pushed 'feat/b' to 'origin'"));

    let feat_a_exists = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/a"])
        .status()
        .expect("verify feat/a push");
    assert!(feat_a_exists.success(), "expected feat/a on remote");

    let feat_b_exists = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/b"])
        .status()
        .expect("verify feat/b push");
    assert!(feat_b_exists.success(), "expected feat/b on remote");
}
