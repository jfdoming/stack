#[test]
fn push_with_no_tracked_branches_does_not_require_placement() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["push"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "no tracked non-base branches to push",
        ));
}

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

#[test]
fn push_skips_merged_branches() {
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

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 11, cached_pr_state = 'merged' WHERE name = 'feat/a'",
        [],
    )
    .expect("seed merged pr cache");

    stack_cmd(repo.path())
        .args(["push"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/a' to 'origin'").not())
        .stdout(predicate::str::contains("pushed 'feat/b' to 'origin'"));

    let feat_a_exists = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/a"])
        .status()
        .expect("verify feat/a not pushed");
    assert!(!feat_a_exists.success(), "expected merged feat/a not to push");

    let feat_b_exists = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/b"])
        .status()
        .expect("verify feat/b push");
    assert!(feat_b_exists.success(), "expected feat/b on remote");
}

#[test]
fn push_uses_configured_upstream_target_for_branches_without_upstream() {
    let repo = init_repo();
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
        &[
            "remote",
            "add",
            "upstream",
            "git@github.com:acme/stack-test.git",
        ],
    );
    run_git(repo.path(), &["config", "branch.main.remote", "upstream"]);
    stack_cmd(repo.path())
        .args(["config", "push-target", "upstream"])
        .assert()
        .success();

    let upstream_push = repo.path().join("upstream-push.git");
    run_git(
        repo.path(),
        &[
            "init",
            "--bare",
            upstream_push.to_str().expect("upstream bare path"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "upstream",
            upstream_push.to_str().expect("upstream bare path"),
        ],
    );
    run_git(repo.path(), &["push", "upstream", "main"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/a"]);
    std::fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["push"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/a' to 'upstream'"));

    let feat_a_exists = Command::new("git")
        .current_dir(&upstream_push)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/a"])
        .status()
        .expect("verify feat/a push");
    assert!(feat_a_exists.success(), "expected feat/a on upstream remote");
}

#[cfg(unix)]
#[test]
fn push_auto_detects_upstream_write_access_independent_of_main_tracking_remote() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &["remote", "set-url", "origin", "git@github.com:alice/stack-test.git"],
    );
    run_git(
        repo.path(),
        &["remote", "add", "upstream", "git@github.com:acme/stack-test.git"],
    );
    run_git(repo.path(), &["config", "branch.main.remote", "origin"]);

    let fork_push = repo.path().join("fork-push.git");
    let upstream_push = repo.path().join("upstream-push.git");
    run_git(repo.path(), &["init", "--bare", fork_push.to_str().unwrap()]);
    run_git(
        repo.path(),
        &["init", "--bare", upstream_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &["remote", "set-url", "--push", "origin", fork_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "upstream",
            upstream_push.to_str().unwrap(),
        ],
    );

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/a", "--name", "feat/b"])
        .assert()
        .success();

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"viewerPermission\"* ]]; then\n  echo '{\"data\":{\"repository\":{\"nameWithOwner\":\"acme/stack-test\",\"viewerPermission\":\"WRITE\",\"parent\":null}}}'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!("{}:{}", fake_bin.display(), env::var("PATH").unwrap_or_default());

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "push"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/a' to 'upstream'"))
        .stdout(predicate::str::contains("pushed 'feat/b' to 'upstream'"));

    for branch in ["feat/a", "feat/b"] {
        let status = Command::new("git")
            .current_dir(&upstream_push)
            .args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")])
            .status()
            .expect("verify upstream branch");
        assert!(status.success(), "expected {branch} on upstream");
    }
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let stored: (Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT push_target, canonical_repo, push_permission FROM repo_meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read auto placement cache");
    assert_eq!(
        stored,
        (
            Some("auto".to_string()),
            Some("acme/stack-test".to_string()),
            Some("WRITE".to_string())
        )
    );
}

#[cfg(unix)]
#[test]
fn push_auto_uses_fork_without_write_access_and_reuses_cached_detection() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &["remote", "set-url", "origin", "git@github.com:alice/stack-test.git"],
    );
    run_git(
        repo.path(),
        &["remote", "add", "upstream", "git@github.com:acme/stack-test.git"],
    );
    let fork_push = repo.path().join("fork-push.git");
    let upstream_push = repo.path().join("upstream-push.git");
    run_git(repo.path(), &["init", "--bare", fork_push.to_str().unwrap()]);
    run_git(
        repo.path(),
        &["init", "--bare", upstream_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &["remote", "set-url", "--push", "origin", fork_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "upstream",
            upstream_push.to_str().unwrap(),
        ],
    );
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    let fake_bin = repo.path().join("fake-bin");
    let query_log = repo.path().join("query.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$*\" == *\"viewerPermission\"* ]]; then\n  echo query >> '{}'\n  echo '{{\"data\":{{\"repository\":{{\"nameWithOwner\":\"acme/stack-test\",\"viewerPermission\":\"READ\",\"parent\":null}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            query_log.display()
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!("{}:{}", fake_bin.display(), env::var("PATH").unwrap_or_default());

    stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["--yes", "push"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/a' to 'origin'"));

    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/b"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "push"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/b' to 'origin'"));

    let queries = fs::read_to_string(query_log).expect("read query log");
    assert_eq!(queries.lines().count(), 1, "permission lookup should be cached");
}

#[test]
fn push_target_override_fails_before_push_when_existing_upstream_conflicts() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &["remote", "set-url", "origin", "git@github.com:alice/stack-test.git"],
    );
    run_git(
        repo.path(),
        &["remote", "add", "upstream", "git@github.com:acme/stack-test.git"],
    );
    let fork_push = repo.path().join("fork-push.git");
    let upstream_push = repo.path().join("upstream-push.git");
    run_git(repo.path(), &["init", "--bare", fork_push.to_str().unwrap()]);
    run_git(
        repo.path(),
        &["init", "--bare", upstream_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &["remote", "set-url", "--push", "origin", fork_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "upstream",
            upstream_push.to_str().unwrap(),
        ],
    );

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    run_git(repo.path(), &["push", "--set-upstream", "origin", "feat/a"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/a", "--name", "feat/b"])
        .assert()
        .success();
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 1, cached_pr_state = 'merged' WHERE name = 'feat/a'",
        [],
    )
    .expect("mark published parent as skipped");

    stack_cmd(repo.path())
        .args(["push", "--push-target", "upstream"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("conflicts with existing upstream"));

    let child_exists = Command::new("git")
        .current_dir(&upstream_push)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/b"])
        .status()
        .expect("verify child not pushed");
    assert!(!child_exists.success());
}

#[cfg(unix)]
#[test]
fn push_target_supports_custom_remote_names() {
    let repo = init_repo_without_origin();
    run_git(
        repo.path(),
        &["remote", "add", "central", "git@github.com:acme/stack-test.git"],
    );
    run_git(
        repo.path(),
        &["remote", "add", "personal", "git@github.com:alice/stack-test.git"],
    );
    let central_push = repo.path().join("central-push.git");
    let personal_push = repo.path().join("personal-push.git");
    run_git(
        repo.path(),
        &["init", "--bare", central_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &["init", "--bare", personal_push.to_str().unwrap()],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "central",
            central_push.to_str().unwrap(),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "personal",
            personal_push.to_str().unwrap(),
        ],
    );
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/custom"])
        .assert()
        .success();

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"viewerPermission\"* ]]; then\n  echo '{\"data\":{\"repository\":{\"nameWithOwner\":\"acme/stack-test\",\"viewerPermission\":\"WRITE\",\"parent\":null}}}'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!("{}:{}", fake_bin.display(), env::var("PATH").unwrap_or_default());

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "push"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "pushed 'feat/custom' to 'central'",
        ));
}
