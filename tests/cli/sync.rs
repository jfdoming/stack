#[test]
fn sync_dry_run_porcelain_reports_restack_operation() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    let old_parent_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "feat/parent"])
            .output()
            .expect("rev-parse parent");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET last_synced_head_sha = ?1 WHERE name = 'feat/parent'",
        [old_parent_sha],
    )
    .expect("seed last synced sha");

    run_git(repo.path(), &["checkout", "feat/parent"]);
    fs::write(repo.path().join("parent.txt"), "parent update\n").expect("write parent change");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent update"]);
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack sync");
    assert!(
        output.status.success(),
        "sync failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let found = ops.iter().any(|op| {
        op["kind"] == "restack" && op["branch"] == "feat/child" && op["onto"] == "feat/parent"
    });
    assert!(found, "expected restack op for feat/child onto feat/parent");
}

#[test]
fn sync_restack_when_parent_not_ancestor_even_without_sha_delta_plans_and_applies() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "main"]);
    fs::write(repo.path().join("base.txt"), "base update\n").expect("write base update");
    run_git(repo.path(), &["add", "base.txt"]);
    run_git(repo.path(), &["commit", "-m", "base update"]);

    let main_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("rev-parse main");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET last_synced_head_sha = ?1 WHERE name = 'main'",
        [main_sha],
    )
    .expect("seed main last synced sha");

    let output = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack sync");
    assert!(
        output.status.success(),
        "sync failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let found_parent = ops.iter().any(|op| {
        op["kind"] == "restack" && op["branch"] == "feat/parent" && op["onto"] == "main"
    });
    assert!(
        found_parent,
        "expected restack op for feat/parent onto main when parent is not ancestor"
    );

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success();

    let parent_contains_main = Command::new("git")
        .current_dir(repo.path())
        .args(["merge-base", "--is-ancestor", "main", "feat/parent"])
        .status()
        .expect("check main ancestor feat/parent");
    assert!(
        parent_contains_main.success(),
        "expected feat/parent to contain main after sync restack"
    );

    let child_contains_parent = Command::new("git")
        .current_dir(repo.path())
        .args(["merge-base", "--is-ancestor", "feat/parent", "feat/child"])
        .status()
        .expect("check parent ancestor feat/child");
    assert!(
        child_contains_parent.success(),
        "expected feat/child to contain feat/parent after sync restack"
    );
}

#[test]
fn sync_fast_forwards_inherited_only_child_without_creating_empty_commit() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/parent"]);
    fs::write(repo.path().join("parent.txt"), "parent work\n").expect("write parent work");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent work"]);
    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path()).args(["sync", "--yes"]).assert().success();

    fs::write(repo.path().join("base.txt"), "base update\n").expect("write base update");
    run_git(repo.path(), &["add", "base.txt"]);
    run_git(repo.path(), &["commit", "-m", "base update"]);

    stack_cmd(repo.path()).args(["sync", "--yes"]).assert().success();

    let parent_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "feat/parent"])
            .output()
            .expect("rev-parse feat/parent");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };
    let child_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "feat/child"])
            .output()
            .expect("rev-parse feat/child");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    assert_eq!(
        child_sha, parent_sha,
        "expected inherited-only child to fast-forward to parent tip"
    );
}
#[test]
fn sync_succeeds_without_origin_remote() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/local"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "no 'origin' remote configured; skipping fetch",
        ));
}

#[test]
fn sync_restores_branch_checked_out_before_run() {
    let replay_supported = {
        let output = Command::new("git")
            .args(["help", "-a"])
            .output()
            .expect("check git help");
        String::from_utf8_lossy(&output.stdout).contains("replay")
    };
    if !replay_supported {
        return;
    }

    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    let old_parent_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "feat/parent"])
            .output()
            .expect("rev-parse parent");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET last_synced_head_sha = ?1 WHERE name = 'feat/parent'",
        [old_parent_sha],
    )
    .expect("seed last synced sha");

    run_git(repo.path(), &["checkout", "feat/parent"]);
    fs::write(repo.path().join("parent.txt"), "parent update\n").expect("write parent change");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent update"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success()
        .stderr(predicate::str::contains("falling back to rebase").not());

    let branch_output = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(branch_output.status.success());
    assert_eq!(
        String::from_utf8(branch_output.stdout)
            .expect("utf8")
            .trim(),
        "main"
    );
}

#[test]
fn sync_plan_fetch_uses_base_branch_remote() {
    let repo = init_repo_with_named_remote("upstream");

    let output = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack sync");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let fetch = ops.first().expect("has first op");
    assert_eq!(fetch["kind"], "fetch");
    assert_eq!(fetch["branch"], "upstream");
}

#[cfg(unix)]
#[test]
fn sync_uses_upstream_and_updates_main_to_merged_commit_not_tip() {
    let repo = init_repo_without_origin();
    let origin_bare = repo.path().join("origin.git");
    let upstream_bare = repo.path().join("upstream.git");

    run_git(
        repo.path(),
        &["init", "--bare", origin_bare.to_str().expect("origin bare")],
    );
    run_git(
        repo.path(),
        &["init", "--bare", upstream_bare.to_str().expect("upstream bare")],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            origin_bare.to_str().expect("origin bare"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "upstream",
            upstream_bare.to_str().expect("upstream bare"),
        ],
    );
    run_git(repo.path(), &["push", "--set-upstream", "origin", "main"]);
    run_git(repo.path(), &["push", "upstream", "main"]);
    run_git(repo.path(), &["config", "branch.main.remote", "origin"]);
    run_git(
        repo.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    let upstream_work = repo.path().join("upstream-work");
    run_git(
        repo.path(),
        &[
            "clone",
            upstream_bare.to_str().expect("upstream bare"),
            upstream_work.to_str().expect("upstream work"),
        ],
    );
    run_git(
        &upstream_work,
        &["config", "user.email", "upstream@example.com"],
    );
    run_git(&upstream_work, &["config", "user.name", "Upstream Bot"]);
    run_git(&upstream_work, &["config", "commit.gpgsign", "false"]);
    fs::write(upstream_work.join("README.md"), "init\nmerged\n").expect("write merged state");
    run_git(&upstream_work, &["add", "README.md"]);
    run_git(&upstream_work, &["commit", "-m", "merge feat/parent"]);
    let merged_sha = {
        let output = Command::new("git")
            .current_dir(&upstream_work)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse merged sha");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    fs::write(upstream_work.join("README.md"), "init\nmerged\nafter\n").expect("write tip state");
    run_git(&upstream_work, &["add", "README.md"]);
    run_git(&upstream_work, &["commit", "-m", "after merge commit"]);
    let upstream_tip_sha = {
        let output = Command::new("git")
            .current_dir(&upstream_work)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse upstream tip");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };
    run_git(&upstream_work, &["push", "origin", "main"]);

    let fake_bin = repo.path().join("fake-bin-merged");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  echo '[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"body\":\"\",\"url\":\"https://github.com/acme/stack-test/pull/11\"}}]'\n  exit 0\nfi\necho '[]'\n",
            merged_sha
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let local_main_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("rev-parse local main");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };
    assert_eq!(
        local_main_sha, merged_sha,
        "expected sync to update local main to merged commit"
    );
    assert_ne!(
        local_main_sha, upstream_tip_sha,
        "expected sync not to advance local main past merged commit"
    );
}

#[cfg(unix)]
#[test]
fn sync_does_not_move_main_without_merged_pr() {
    let repo = init_repo_without_origin();
    let origin_bare = repo.path().join("origin.git");
    let upstream_bare = repo.path().join("upstream.git");

    run_git(
        repo.path(),
        &["init", "--bare", origin_bare.to_str().expect("origin bare")],
    );
    run_git(
        repo.path(),
        &["init", "--bare", upstream_bare.to_str().expect("upstream bare")],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            origin_bare.to_str().expect("origin bare"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "upstream",
            upstream_bare.to_str().expect("upstream bare"),
        ],
    );
    run_git(repo.path(), &["push", "--set-upstream", "origin", "main"]);
    run_git(repo.path(), &["push", "upstream", "main"]);
    run_git(repo.path(), &["config", "branch.main.remote", "origin"]);
    run_git(
        repo.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );

    let main_before_sync = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("rev-parse main before sync");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    let upstream_work = repo.path().join("upstream-work-no-merge");
    run_git(
        repo.path(),
        &[
            "clone",
            upstream_bare.to_str().expect("upstream bare"),
            upstream_work.to_str().expect("upstream work"),
        ],
    );
    run_git(
        &upstream_work,
        &["config", "user.email", "upstream@example.com"],
    );
    run_git(&upstream_work, &["config", "user.name", "Upstream Bot"]);
    run_git(&upstream_work, &["config", "commit.gpgsign", "false"]);
    fs::write(upstream_work.join("README.md"), "init\nupstream only\n").expect("write upstream");
    run_git(&upstream_work, &["add", "README.md"]);
    run_git(&upstream_work, &["commit", "-m", "upstream only"]);
    run_git(&upstream_work, &["push", "origin", "main"]);

    let fake_bin = repo.path().join("fake-bin-no-merge");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  echo '[]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let main_after_sync = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("rev-parse main after sync");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };
    assert_eq!(
        main_after_sync, main_before_sync,
        "expected main to remain unchanged when no PR is merged"
    );
}

#[cfg(unix)]
#[test]
fn sync_updates_existing_pr_body_with_managed_section() {
    let repo = init_repo_without_origin();
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:acme/stack-test.git",
        ],
    );
    run_git(repo.path(), &["config", "branch.main.remote", "no-fetch"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    let fake_bin = repo.path().join("fake-bin");
    let gh_log = repo.path().join("gh.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  if [[ \"$*\" == *\"headRefName\"* ]]; then\n    echo '[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"mergeCommit\":null,\"body\":\"Existing reviewer notes\"}}]'\n    exit 0\n  fi\n  for ((i=1; i<=$#; i++)); do\n    if [[ \"${{!i}}\" == \"--head\" ]]; then\n      next=$((i+1))\n      head=\"${{!next}}\"\n      break\n    fi\n  done\n  if [[ \"$head\" == \"feat/child\" ]]; then\n    echo '[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"mergeCommit\":null,\"body\":\"Existing reviewer notes\"}}]'\n  else\n    echo '[]'\n  fi\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" ]]; then\n  exit 0\nfi\necho '[]'\n",
            gh_log.display()
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 6944, cached_pr_state = 'open' WHERE name = 'feat/parent'",
        [],
    )
    .expect("seed stale parent pr cache");

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let gh_calls = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(
        gh_calls.contains("pr edit 42 --body"),
        "expected pr edit call for managed body refresh, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("pr list --state all --limit 200"),
        "expected batched pr list metadata request, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("stack:managed:start"),
        "expected managed section start marker in edited body, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("feat/parent"),
        "expected parent reference in edited body, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("/tree/feat/parent"),
        "expected unresolved parent to link to branch path, got: {gh_calls}"
    );
    assert!(
        !gh_calls.contains("/pull/6944"),
        "expected stale cached parent PR not to be reused, got: {gh_calls}"
    );
}

#[test]
fn sync_yes_does_not_push_in_non_interactive_mode() {
    let repo = init_repo_without_origin();
    let bare = repo.path().join("origin.git");
    run_git(
        repo.path(),
        &["init", "--bare", bare.to_str().expect("bare path")],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            bare.to_str().expect("bare path"),
        ],
    );
    run_git(repo.path(), &["config", "branch.main.remote", "origin"]);
    run_git(
        repo.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    run_git(repo.path(), &["push", "--set-upstream", "origin", "main"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/local"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/local"]);
    fs::write(repo.path().join("sync-push.txt"), "local\n").expect("write sync push file");
    run_git(repo.path(), &["add", "sync-push.txt"]);
    run_git(repo.path(), &["commit", "-m", "sync push commit"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/local' to 'origin'").not());

    let pushed = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/local"])
        .status()
        .expect("verify branch not pushed");
    assert!(
        !pushed.success(),
        "expected non-interactive sync --yes not to auto-push"
    );
}

#[test]
fn sync_without_yes_does_not_push_in_non_interactive_mode() {
    let repo = init_repo_without_origin();
    let bare = repo.path().join("origin.git");
    run_git(
        repo.path(),
        &["init", "--bare", bare.to_str().expect("bare path")],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            bare.to_str().expect("bare path"),
        ],
    );
    run_git(repo.path(), &["config", "branch.main.remote", "origin"]);
    run_git(
        repo.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    run_git(repo.path(), &["push", "--set-upstream", "origin", "main"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/local"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/local"]);
    fs::write(repo.path().join("sync-no-yes.txt"), "local\n").expect("write sync no yes file");
    run_git(repo.path(), &["add", "sync-no-yes.txt"]);
    run_git(repo.path(), &["commit", "-m", "sync no yes commit"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path()).args(["sync"]).assert().success();

    let pushed = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/local"])
        .status()
        .expect("verify branch not pushed");
    assert!(
        !pushed.success(),
        "expected non-interactive sync without --yes not to push"
    );
}
