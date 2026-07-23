#[test]
fn sync_dry_run_porcelain_reports_restack_operation() {
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
fn sync_fetches_and_restacks_onto_an_advanced_remote_base_in_one_run() {
    let repo = init_repo_without_origin();
    let upstream_bare = repo.path().join("upstream.git");
    run_git(
        repo.path(),
        &[
            "init",
            "--bare",
            upstream_bare.to_str().expect("upstream bare path"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "upstream",
            upstream_bare.to_str().expect("upstream bare path"),
        ],
    );
    run_git(
        repo.path(),
        &["push", "--set-upstream", "upstream", "main"],
    );

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    fs::write(repo.path().join("root.txt"), "root work\n").expect("write root work");
    run_git(repo.path(), &["add", "root.txt"]);
    run_git(repo.path(), &["commit", "-m", "root work"]);
    run_git(repo.path(), &["checkout", "main"]);
    let local_main = git_ref_sha(repo.path(), "refs/heads/main").expect("local main");

    let upstream_work = repo.path().join(".git/test-upstream-work");
    run_git(
        repo.path(),
        &[
            "clone",
            "--branch",
            "main",
            upstream_bare.to_str().expect("upstream bare path"),
            upstream_work.to_str().expect("upstream work path"),
        ],
    );
    run_git(
        &upstream_work,
        &["config", "user.email", "upstream@example.com"],
    );
    run_git(&upstream_work, &["config", "user.name", "Upstream Bot"]);
    run_git(&upstream_work, &["config", "commit.gpgsign", "false"]);
    fs::write(upstream_work.join("base.txt"), "advanced base\n").expect("write base change");
    run_git(&upstream_work, &["add", "base.txt"]);
    run_git(&upstream_work, &["commit", "-m", "advance remote base"]);
    let remote_main = git_ref_sha(&upstream_work, "HEAD").expect("remote main");
    run_git(&upstream_work, &["push", "origin", "main"]);

    assert_eq!(
        git_ref_sha(repo.path(), "refs/remotes/upstream/main"),
        Some(local_main.clone()),
        "test requires a stale remote-tracking ref"
    );

    let dry_run = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("plan sync against advanced remote base");
    assert!(
        dry_run.status.success(),
        "sync planning failed: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let plan: Value = serde_json::from_slice(&dry_run.stdout).expect("valid sync plan");
    let operations = plan["operations"].as_array().expect("operations array");
    assert!(
        operations
            .iter()
            .any(|op| op["kind"] == "fetch" && op["branch"] == "upstream"),
        "expected the stale upstream ref to be fetched: {operations:?}"
    );
    assert!(
        operations.iter().any(|op| {
            op["kind"] == "restack"
                && op["branch"] == "feat/root"
                && op["onto"] == remote_main
        }),
        "expected the root branch to target the advertised remote base: {operations:?}"
    );

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success();

    let contains_remote_base = Command::new("git")
        .current_dir(repo.path())
        .args([
            "merge-base",
            "--is-ancestor",
            &remote_main,
            "feat/root",
        ])
        .status()
        .expect("check remote base ancestry");
    assert!(
        contains_remote_base.success(),
        "expected feat/root to contain the advertised remote base"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/main"),
        Some(local_main),
        "an unmerged remote advance must not move local main"
    );

    let repeated = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("plan repeated sync");
    assert!(repeated.status.success());
    let repeated_plan: Value =
        serde_json::from_slice(&repeated.stdout).expect("valid repeated sync plan");
    let repeated_operations = repeated_plan["operations"]
        .as_array()
        .expect("repeated operations array");
    assert!(
        !repeated_operations
            .iter()
            .any(|op| op["kind"] == "restack" && op["branch"] == "feat/root"),
        "expected the remote-base restack to be idempotent: {repeated_operations:?}"
    );
}

#[cfg(unix)]
#[test]
fn sync_refuses_a_restack_when_the_child_changes_after_planning() {
    let repo = init_repo_without_origin();
    let upstream_bare = repo.path().join("upstream-race.git");
    run_git(
        repo.path(),
        &[
            "init",
            "--bare",
            upstream_bare.to_str().expect("upstream bare path"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "upstream",
            upstream_bare.to_str().expect("upstream bare path"),
        ],
    );
    run_git(
        repo.path(),
        &["push", "--set-upstream", "upstream", "main"],
    );

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    fs::write(repo.path().join("root.txt"), "root work\n").expect("write root work");
    run_git(repo.path(), &["add", "root.txt"]);
    run_git(repo.path(), &["commit", "-m", "root work"]);
    fs::write(repo.path().join("concurrent.txt"), "concurrent work\n")
        .expect("write concurrent work");
    run_git(repo.path(), &["add", "concurrent.txt"]);
    run_git(repo.path(), &["commit", "-m", "concurrent work"]);
    let concurrent_head =
        git_ref_sha(repo.path(), "refs/heads/feat/root").expect("concurrent head");
    run_git(repo.path(), &["reset", "--hard", "HEAD^"]);
    let planned_head =
        git_ref_sha(repo.path(), "refs/heads/feat/root").expect("planned child head");
    run_git(repo.path(), &["checkout", "main"]);

    let upstream_work = repo.path().join(".git/test-upstream-race-work");
    run_git(
        repo.path(),
        &[
            "clone",
            "--branch",
            "main",
            upstream_bare.to_str().expect("upstream bare path"),
            upstream_work.to_str().expect("upstream work path"),
        ],
    );
    run_git(
        &upstream_work,
        &["config", "user.email", "upstream@example.com"],
    );
    run_git(&upstream_work, &["config", "user.name", "Upstream Bot"]);
    run_git(
        &upstream_work,
        &["config", "commit.gpgsign", "false"],
    );
    fs::write(upstream_work.join("base.txt"), "advanced base\n").expect("write base change");
    run_git(&upstream_work, &["add", "base.txt"]);
    run_git(&upstream_work, &["commit", "-m", "advance remote base"]);
    run_git(&upstream_work, &["push", "origin", "main"]);

    let fake_bin = repo.path().join("fake-bin-restack-race");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let real_git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .expect("resolve real git");
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout)
        .expect("utf8 git path")
        .trim()
        .to_string();
    let replay_supported = Command::new(&real_git)
        .args(["help", "-a"])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("replay"));
    let replay_marker = repo.path().join("replay-race.log");
    let rebase_marker = repo.path().join("rebase-race.log");
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"help\" && \"$2\" == \"-a\" && \"${{STACK_TEST_FORCE_REBASE:-}}\" == \"1\" ]]; then\n  '{}' help -a | sed '/replay/d'\n  exit 0\nfi\nif [[ \"$1\" == \"replay\" ]]; then\n  : > '{}'\n  replay_output=$('{}' \"$@\")\n  status=$?\n  if [[ $status -eq 0 ]]; then\n    '{}' update-ref refs/heads/feat/root '{}'\n  fi\n  printf '%s\\n' \"$replay_output\"\n  exit $status\nfi\nif [[ \"$1\" == \"rebase\" ]]; then\n  : > '{}'\n  '{}' \"$@\"\n  status=$?\n  if [[ $status -eq 0 ]]; then\n    '{}' update-ref refs/heads/feat/root '{}'\n  fi\n  exit $status\nfi\nexec '{}' \"$@\"\n",
            real_git,
            replay_marker.display(),
            real_git,
            real_git,
            concurrent_head,
            rebase_marker.display(),
            real_git,
            real_git,
            concurrent_head,
            real_git
        ),
    )
    .expect("write fake git");
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "branch 'feat/root' changed after the sync plan was built",
        ));
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/root"),
        Some(concurrent_head.clone())
    );
    if replay_supported {
        assert!(
            replay_marker.exists(),
            "expected the race to exercise git replay"
        );
    }

    run_git(
        repo.path(),
        &["update-ref", "refs/heads/feat/root", &planned_head],
    );
    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env("STACK_TEST_FORCE_REBASE", "1")
        .args(["sync", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "branch 'feat/root' changed after the sync plan was built",
        ));
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/root"),
        Some(concurrent_head)
    );
    assert!(
        rebase_marker.exists(),
        "expected the race to exercise the rebase fallback"
    );
}

#[cfg(unix)]
#[test]
fn sync_finishes_a_conflicted_atomic_restack_after_rebase_continue() {
    let repo = init_repo_without_origin();
    let upstream_bare = repo.path().join("upstream-conflict.git");
    run_git(
        repo.path(),
        &[
            "init",
            "--bare",
            upstream_bare.to_str().expect("upstream bare path"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "upstream",
            upstream_bare.to_str().expect("upstream bare path"),
        ],
    );
    run_git(
        repo.path(),
        &["push", "--set-upstream", "upstream", "main"],
    );

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    fs::write(repo.path().join("README.md"), "child version\n").expect("write child change");
    run_git(repo.path(), &["add", "README.md"]);
    run_git(repo.path(), &["commit", "-m", "change child readme"]);
    run_git(repo.path(), &["checkout", "main"]);

    let upstream_work = repo.path().join(".git/test-upstream-conflict-work");
    run_git(
        repo.path(),
        &[
            "clone",
            "--branch",
            "main",
            upstream_bare.to_str().expect("upstream bare path"),
            upstream_work.to_str().expect("upstream work path"),
        ],
    );
    run_git(
        &upstream_work,
        &["config", "user.email", "upstream@example.com"],
    );
    run_git(&upstream_work, &["config", "user.name", "Upstream Bot"]);
    run_git(
        &upstream_work,
        &["config", "commit.gpgsign", "false"],
    );
    fs::write(upstream_work.join("README.md"), "upstream version\n")
        .expect("write upstream change");
    run_git(&upstream_work, &["add", "README.md"]);
    run_git(&upstream_work, &["commit", "-m", "change upstream readme"]);
    run_git(&upstream_work, &["push", "origin", "main"]);

    let fake_bin = repo.path().join("fake-bin-conflict-recovery");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let real_git = Command::new("sh")
        .args(["-lc", "command -v git"])
        .output()
        .expect("resolve real git");
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout)
        .expect("utf8 git path")
        .trim()
        .to_string();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"help\" && \"$2\" == \"-a\" ]]; then\n  '{}' help -a | sed '/replay/d'\n  exit 0\nfi\nexec '{}' \"$@\"\n",
            real_git, real_git
        ),
    )
    .expect("write fake git");
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "sync stopped due to merge conflicts while restacking 'feat/root'",
        ));

    fs::write(repo.path().join("README.md"), "resolved version\n")
        .expect("resolve readme conflict");
    run_git(repo.path(), &["add", "README.md"]);
    run_git(
        repo.path(),
        &["-c", "core.editor=true", "rebase", "--continue"],
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--yes"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "finished resolved restack for 'feat/root'",
        ));

    assert_eq!(
        fs::read_to_string(repo.path().join("README.md")).expect("read resolved file"),
        "resolved version\n"
    );
    let contains_remote_base = Command::new("git")
        .current_dir(repo.path())
        .args([
            "merge-base",
            "--is-ancestor",
            "refs/remotes/upstream/main",
            "feat/root",
        ])
        .status()
        .expect("check restacked ancestry");
    assert!(contains_remote_base.success());
    let recovery_branches = Command::new("git")
        .current_dir(repo.path())
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/stack/restack",
        ])
        .output()
        .expect("list recovery branches");
    assert!(recovery_branches.status.success());
    assert!(recovery_branches.stdout.is_empty());
    let pending_refs = Command::new("git")
        .current_dir(repo.path())
        .args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/stack/restacks",
        ])
        .output()
        .expect("list pending restack refs");
    assert!(pending_refs.status.success());
    assert!(pending_refs.stdout.is_empty());
}

#[test]
fn sync_rejects_a_forged_restack_recovery_branch() {
    let repo = init_repo_without_origin();
    let main_head = git_ref_sha(repo.path(), "refs/heads/main").expect("main head");
    let forged_branch = format!("stack/restack/1-1/{main_head}/main");
    run_git(repo.path(), &["branch", &forged_branch, "main"]);
    run_git(repo.path(), &["checkout", &forged_branch]);
    fs::write(repo.path().join("forged.txt"), "forged recovery\n")
        .expect("write forged commit");
    run_git(repo.path(), &["add", "forged.txt"]);
    run_git(repo.path(), &["commit", "-m", "forged recovery commit"]);

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "refusing to finish untrusted restack recovery branch",
        ));

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/main"),
        Some(main_head)
    );
}

#[test]
fn sync_inspects_an_origin_base_without_an_upstream_or_tracking_ref() {
    let repo = init_repo_without_origin();
    let origin_bare = repo.path().join("origin.git");
    run_git(
        repo.path(),
        &[
            "init",
            "--bare",
            origin_bare.to_str().expect("origin bare path"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            origin_bare.to_str().expect("origin bare path"),
        ],
    );
    run_git(repo.path(), &["push", "origin", "main"]);
    run_git(
        repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    fs::write(repo.path().join("root.txt"), "root work\n").expect("write root work");
    run_git(repo.path(), &["add", "root.txt"]);
    run_git(repo.path(), &["commit", "-m", "root work"]);
    run_git(repo.path(), &["checkout", "main"]);
    let local_main = git_ref_sha(repo.path(), "refs/heads/main").expect("local main");

    let origin_work = repo.path().join(".git/test-origin-work");
    run_git(
        repo.path(),
        &[
            "clone",
            "--branch",
            "main",
            origin_bare.to_str().expect("origin bare path"),
            origin_work.to_str().expect("origin work path"),
        ],
    );
    run_git(
        &origin_work,
        &["config", "user.email", "origin@example.com"],
    );
    run_git(&origin_work, &["config", "user.name", "Origin Bot"]);
    run_git(&origin_work, &["config", "commit.gpgsign", "false"]);
    fs::write(origin_work.join("base.txt"), "advanced base\n").expect("write base change");
    run_git(&origin_work, &["add", "base.txt"]);
    run_git(&origin_work, &["commit", "-m", "advance remote base"]);
    let remote_main = git_ref_sha(&origin_work, "HEAD").expect("remote main");
    run_git(&origin_work, &["push", "origin", "main"]);

    assert!(
        git_ref_sha(repo.path(), "refs/remotes/origin/main").is_none(),
        "test requires the remote-tracking ref to be absent"
    );
    let base_upstream = Command::new("git")
        .current_dir(repo.path())
        .args(["rev-parse", "--abbrev-ref", "main@{upstream}"])
        .output()
        .expect("inspect base upstream");
    assert!(
        !base_upstream.status.success(),
        "test requires main to have no upstream"
    );

    let dry_run = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("plan sync without local remote-base state");
    assert!(
        dry_run.status.success(),
        "sync planning failed: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let plan: Value = serde_json::from_slice(&dry_run.stdout).expect("valid sync plan");
    let operations = plan["operations"].as_array().expect("operations array");
    assert!(
        operations
            .iter()
            .any(|op| op["kind"] == "fetch" && op["branch"] == "origin"),
        "expected origin to be fetched despite the missing tracking ref: {operations:?}"
    );
    assert!(
        operations.iter().any(|op| {
            op["kind"] == "restack"
                && op["branch"] == "feat/root"
                && op["onto"] == remote_main
        }),
        "expected the root branch to target the advertised origin base: {operations:?}"
    );

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success();
    let contains_remote_base = Command::new("git")
        .current_dir(repo.path())
        .args([
            "merge-base",
            "--is-ancestor",
            &remote_main,
            "feat/root",
        ])
        .status()
        .expect("check remote base ancestry");
    assert!(contains_remote_base.success());
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/main"),
        Some(local_main),
        "an unmerged remote advance must not move local main"
    );

    let repeated = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("plan repeated sync");
    assert!(repeated.status.success());
    let repeated_plan: Value =
        serde_json::from_slice(&repeated.stdout).expect("valid repeated sync plan");
    assert!(
        !repeated_plan["operations"]
            .as_array()
            .expect("repeated operations array")
            .iter()
            .any(|op| op["kind"] == "restack" && op["branch"] == "feat/root"),
        "expected the origin-base restack to be idempotent"
    );
}

#[test]
fn sync_drops_obsolete_parent_commits_after_parent_rewrite() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    fs::write(repo.path().join("old-parent.txt"), "obsolete parent\n")
        .expect("write old parent work");
    run_git(repo.path(), &["add", "old-parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "obsolete parent commit"]);

    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "feat/parent",
            "--name",
            "feat/child",
        ])
        .assert()
        .success();
    fs::write(repo.path().join("child.txt"), "child work\n").expect("write child work");
    run_git(repo.path(), &["add", "child.txt"]);
    run_git(repo.path(), &["commit", "-m", "child commit"]);

    run_git(repo.path(), &["checkout", "feat/parent"]);
    run_git(repo.path(), &["reset", "--hard", "main"]);
    fs::write(repo.path().join("new-parent.txt"), "replacement parent\n")
        .expect("write replacement parent work");
    run_git(repo.path(), &["add", "new-parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "replacement parent commit"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success();

    let parent_is_ancestor = Command::new("git")
        .current_dir(repo.path())
        .args([
            "merge-base",
            "--is-ancestor",
            "feat/parent",
            "feat/child",
        ])
        .status()
        .expect("check rewritten parent ancestry");
    assert!(parent_is_ancestor.success());

    let child_log = Command::new("git")
        .current_dir(repo.path())
        .args(["log", "--format=%s", "feat/parent..feat/child"])
        .output()
        .expect("read child-only commits");
    assert!(child_log.status.success());
    let child_subjects = String::from_utf8(child_log.stdout).expect("utf8 child log");
    assert!(child_subjects.contains("child commit"));
    assert!(
        !child_subjects.contains("obsolete parent commit"),
        "obsolete parent history was duplicated into the child: {child_subjects}"
    );

    let old_parent_file = Command::new("git")
        .current_dir(repo.path())
        .args(["cat-file", "-e", "feat/child:old-parent.txt"])
        .output()
        .expect("inspect child tree");
    assert!(
        !old_parent_file.status.success(),
        "obsolete parent content must not be replayed into the child"
    );

    let repeated = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("plan repeated parent-rewrite sync");
    assert!(repeated.status.success());
    let repeated_plan: Value =
        serde_json::from_slice(&repeated.stdout).expect("valid repeated sync plan");
    assert!(
        !repeated_plan["operations"]
            .as_array()
            .expect("repeated operations array")
            .iter()
            .any(|op| op["kind"] == "restack" && op["branch"] == "feat/child")
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
        .success();
}

#[test]
fn sync_debug_prints_timing_info() {
    let repo = init_repo_without_origin();

    let output = stack_cmd(repo.path())
        .args(["--debug", "sync", "--dry-run"])
        .output()
        .expect("run stack sync");
    assert!(
        output.status.success(),
        "sync failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("debug: sync timing"),
        "expected debug timing output, got: {stderr}"
    );
    assert!(
        stderr.contains("plan_ms="),
        "expected plan timing metric, got: {stderr}"
    );
    assert!(
        stderr.contains("total_ms="),
        "expected total timing metric, got: {stderr}"
    );
    assert!(
        stderr.contains("pr_lookup_ms="),
        "expected detailed plan timing metric, got: {stderr}"
    );
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
fn sync_plan_omits_noop_fetch_and_updates_when_stack_is_current() {
    let repo = init_repo_with_named_remote("upstream");

    let output = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack sync");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    assert!(
        ops.is_empty(),
        "expected no sync operations when stack is already current: {ops:?}"
    );
}

#[test]
fn sync_skips_apply_when_plan_has_no_operations() {
    let repo = init_repo_with_named_remote("upstream");

    stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .assert()
        .success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    let before_runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM sync_runs", [], |row| row.get(0))
        .expect("count sync runs before");

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync already up to date"));

    let after_runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM sync_runs", [], |row| row.get(0))
        .expect("count sync runs after");
    assert_eq!(
        before_runs, after_runs,
        "expected sync with no operations to skip execution bookkeeping"
    );
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

    let local_main_before_pull = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("rev-parse local main before pull");
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
        [local_main_before_pull],
    )
    .expect("seed main last synced sha");

    let upstream_work = repo.path().join("upstream-work");
    run_git(
        repo.path(),
        &[
            "clone",
            "--branch",
            "main",
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
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[]}},\"h1\":{{\"nodes\":[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            merged_sha
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let preflight = stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run preflight sync dry-run");
    assert!(preflight.status.success());
    let preflight_json: Value = serde_json::from_slice(&preflight.stdout).expect("valid json");
    let preflight_ops = preflight_json["operations"]
        .as_array()
        .expect("operations array");
    let fetch = preflight_ops.first().expect("has fetch op");
    assert_eq!(fetch["kind"], "fetch");
    assert_eq!(fetch["branch"], "upstream");
    let merged_parent_restack = preflight_ops
        .iter()
        .any(|op| op["kind"] == "restack" && op["branch"] == "feat/parent");
    assert!(
        !merged_parent_restack,
        "expected merged parent branch to be excluded from restack planning"
    );

    stack_cmd(repo.path())
        .env("PATH", &test_path)
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

    let output = stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run second sync dry-run");
    assert!(
        output.status.success(),
        "second sync dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let child_restack = ops.iter().any(|op| {
        op["kind"] == "restack" && op["branch"] == "feat/child" && op["onto"] == merged_sha
    });
    assert!(
        !child_restack,
        "expected no repeated child restack once already based on merged commit"
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
            "--branch",
            "main",
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
fn sync_rebase_fallback_drops_merged_parent_commits_after_squash_merge() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/parent"]);
    fs::write(repo.path().join("parent.txt"), "p1\n").expect("write parent p1");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent 1"]);
    fs::write(repo.path().join("parent.txt"), "p1\np2\n").expect("write parent p2");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent 2"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/child"]);
    fs::write(repo.path().join("child.txt"), "c1\n").expect("write child c1");
    run_git(repo.path(), &["add", "child.txt"]);
    run_git(repo.path(), &["commit", "-m", "child 1"]);
    fs::write(repo.path().join("child.txt"), "c1\nc2\n").expect("write child c2");
    run_git(repo.path(), &["add", "child.txt"]);
    run_git(repo.path(), &["commit", "-m", "child 2"]);

    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["merge", "--squash", "feat/parent"]);
    run_git(repo.path(), &["commit", "-m", "squash parent"]);
    let merged_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse merged sha");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    let fake_bin = repo.path().join("fake-bin-rebase-fallback");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let real_git = {
        let output = Command::new("sh")
            .args(["-lc", "command -v git"])
            .output()
            .expect("resolve real git");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"help\" && \"$2\" == \"-a\" ]]; then\n  \"{}\" help -a | sed '/replay/d'\n  exit 0\nfi\nexec \"{}\" \"$@\"\n",
            real_git, real_git
        ),
    )
    .expect("write fake git");
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");

    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[]}},\"h1\":{{\"nodes\":[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
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

    let child_subjects = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args([
                "log",
                "--format=%s",
                &format!("{merged_sha}..feat/child"),
            ])
            .output()
            .expect("child log");
        assert!(output.status.success());
        String::from_utf8(output.stdout).expect("utf8")
    };

    assert!(
        child_subjects.contains("child 1"),
        "expected child commit to remain after sync: {child_subjects}"
    );
    assert!(
        child_subjects.contains("child 2"),
        "expected child commit to remain after sync: {child_subjects}"
    );
    assert!(
        !child_subjects.contains("parent 1"),
        "expected merged parent commit to be dropped after sync: {child_subjects}"
    );
    assert!(
        !child_subjects.contains("parent 2"),
        "expected merged parent commit to be dropped after sync: {child_subjects}"
    );
}

#[cfg(unix)]
#[test]
fn sync_skips_cached_merged_branch_when_pr_metadata_missing() {
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
    fs::write(repo.path().join("parent.txt"), "parent\n").expect("write parent change");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent change"]);

    run_git(repo.path(), &["checkout", "main"]);
    fs::write(repo.path().join("base.txt"), "base update\n").expect("write base update");
    run_git(repo.path(), &["add", "base.txt"]);
    run_git(repo.path(), &["commit", "-m", "base update"]);

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 11, cached_pr_state = 'merged' WHERE name = 'feat/parent'",
        [],
    )
    .expect("seed merged pr cache");

    let fake_bin = repo.path().join("fake-bin-no-pr-metadata");
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

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run sync dry-run");
    assert!(
        output.status.success(),
        "sync dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let parent_restack = ops.iter().any(|op| {
        op["kind"] == "restack" && op["branch"] == "feat/parent" && op["onto"] == "main"
    });
    assert!(
        !parent_restack,
        "expected merged parent branch to be skipped from restack operations"
    );
}

#[cfg(unix)]
#[test]
fn sync_does_not_restack_cached_merged_direct_child_when_base_sha_changes() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    let old_main_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("rev-parse main before update");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    fs::write(repo.path().join("base.txt"), "base update\n").expect("write base update");
    run_git(repo.path(), &["add", "base.txt"]);
    run_git(repo.path(), &["commit", "-m", "base update"]);

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET last_synced_head_sha = ?1 WHERE name = 'main'",
        [old_main_sha],
    )
    .expect("seed main last synced sha");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 11, cached_pr_state = 'merged' WHERE name = 'feat/parent'",
        [],
    )
    .expect("seed merged parent cache");

    let fake_bin = repo.path().join("fake-bin-no-pr-metadata-merged-child");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run sync dry-run");
    assert!(
        output.status.success(),
        "sync dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let parent_restack = ops
        .iter()
        .any(|op| op["kind"] == "restack" && op["branch"] == "feat/parent");
    assert!(
        !parent_restack,
        "expected merged direct child to be excluded from restack operations when base SHA changes"
    );
    assert!(
        !ops
            .iter()
            .any(|op| op["kind"] == "prune_merged" && op["branch"] == "feat/parent"),
        "cached merged state without a fresh PR head must not authorize pruning"
    );
}

#[cfg(unix)]
#[test]
fn sync_updates_base_sha_when_base_already_contains_merge_commit() {
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
    run_git(repo.path(), &["checkout", "main"]);

    let upstream_work = repo.path().join("upstream-work-contains-merge");
    run_git(
        repo.path(),
        &[
            "clone",
            "--branch",
            "main",
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
    run_git(&upstream_work, &["push", "origin", "main"]);

    run_git(repo.path(), &["fetch", "upstream"]);
    run_git(repo.path(), &["merge", "--ff-only", "upstream/main"]);

    let current_main_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("rev-parse current main sha");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/active"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET last_synced_head_sha = ?1 WHERE name = 'main'",
        [&merged_sha],
    )
    .expect("seed stale main sync sha");

    let fake_bin = repo.path().join("fake-bin-skip-update-base");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            merged_sha
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let output = stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run sync dry-run");
    assert!(
        output.status.success(),
        "sync dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let has_update_base = ops.iter().any(|op| op["kind"] == "update_base");
    assert!(
        !has_update_base,
        "expected no update_base op when base already contains merged commit"
    );
    let has_update_sha = ops.iter().any(|op| {
        op["kind"] == "update_sha"
            && op["branch"] == "main"
            && op["details"] == current_main_sha
    });
    assert!(
        has_update_sha,
        "expected current base SHA to be persisted when it already contains the merge commit"
    );

    stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let stored_main_sha: String = conn
        .query_row(
            "SELECT last_synced_head_sha FROM branches WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .expect("read stored main sync sha");
    assert_eq!(stored_main_sha, current_main_sha);

    let repeated = stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run repeated sync dry-run");
    assert!(
        repeated.status.success(),
        "repeated sync dry-run failed: {}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated_json: Value =
        serde_json::from_slice(&repeated.stdout).expect("valid repeated sync json");
    let repeated_ops = repeated_json["operations"]
        .as_array()
        .expect("repeated operations array");
    assert!(
        repeated_ops.is_empty(),
        "expected no repeated sync operations, got {repeated_ops:?}"
    );
}

#[cfg(unix)]
#[test]
fn sync_does_not_restack_child_when_merged_parent_ref_is_missing() {
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

    run_git(repo.path(), &["branch", "-D", "feat/parent"]);

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 11, cached_pr_state = 'merged' WHERE name = 'feat/parent'",
        [],
    )
    .expect("seed merged parent cache");

    let fake_bin = repo.path().join("fake-bin-no-parent-ref-restack");
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

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run sync dry-run");
    assert!(
        output.status.success(),
        "sync dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let child_restack = ops
        .iter()
        .any(|op| op["kind"] == "restack" && op["branch"] == "feat/child");
    assert!(
        !child_restack,
        "expected no child restack when merged parent ref is missing locally"
    );
}

#[cfg(unix)]
#[test]
fn sync_preserves_the_entire_stack_when_a_merged_branch_has_later_commits() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/merged"])
        .assert()
        .success();
    fs::write(repo.path().join("merged.txt"), "merged content\n").expect("write merged content");
    run_git(repo.path(), &["add", "merged.txt"]);
    run_git(repo.path(), &["commit", "-m", "merged PR head"]);
    let merged_parent_head =
        git_ref_sha(repo.path(), "refs/heads/feat/merged").expect("merged parent head");
    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "feat/merged",
            "--name",
            "feat/child",
        ])
        .assert()
        .success();
    let merged_child_head =
        git_ref_sha(repo.path(), "refs/heads/feat/child").expect("merged child head");

    fs::write(repo.path().join("later.txt"), "post-merge work\n")
        .expect("write post-merge content");
    run_git(repo.path(), &["add", "later.txt"]);
    run_git(repo.path(), &["commit", "-m", "post-merge work"]);
    let post_merge_head =
        git_ref_sha(repo.path(), "refs/heads/feat/child").expect("post-merge head");
    run_git(repo.path(), &["checkout", "main"]);
    let main_sha = git_ref_sha(repo.path(), "refs/heads/main").expect("main head");

    let fake_bin = repo.path().join("fake-bin-preserve-post-merge");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":12,\"state\":\"MERGED\",\"baseRefName\":\"feat/merged\",\"headRefName\":\"feat/child\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/12\",\"body\":\"\"}}]}},\"h1\":{{\"nodes\":[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/merged\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            merged_child_head, main_sha, merged_parent_head, main_sha
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/child"),
        Some(post_merge_head)
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/merged"),
        Some(merged_parent_head)
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let record_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name IN ('feat/merged', 'feat/child')",
            [],
            |row| row.get(0),
        )
        .expect("count preserved branch record");
    assert_eq!(record_count, 2);
}

#[cfg(unix)]
#[test]
fn sync_refuses_to_prune_the_dirty_checked_out_branch() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/merged"])
        .assert()
        .success();
    let merged_head = git_ref_sha(repo.path(), "refs/heads/feat/merged").expect("merged head");
    fs::write(repo.path().join("README.md"), "uncommitted work\n")
        .expect("write dirty tracked file");

    let fake_bin = repo.path().join("fake-bin-dirty-prune");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/merged\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            merged_head, merged_head
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--yes"])
        .output()
        .expect("run sync with dirty merged branch checked out");

    assert!(
        git_ref_sha(repo.path(), "refs/heads/feat/merged").is_some(),
        "dirty starting branch must not be pruned"
    );
    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert_eq!(String::from_utf8_lossy(&current.stdout).trim(), "feat/merged");
    assert_eq!(
        fs::read_to_string(repo.path().join("README.md")).expect("read dirty file"),
        "uncommitted work\n"
    );
    let stash_list = Command::new("git")
        .current_dir(repo.path())
        .args(["stash", "list"])
        .output()
        .expect("list stashes");
    assert!(
        stash_list.stdout.is_empty(),
        "sync must refuse before creating an auto-stash"
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("cannot prune the checked-out branch with uncommitted changes"),
        "unexpected error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn sync_prunes_fully_merged_stack_branches() {
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

    let fake_bin = repo.path().join("fake-bin-prune-merged");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":12,\"state\":\"MERGED\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/12\",\"body\":\"\"}}]}},\"h1\":{{\"nodes\":[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            main_sha, main_sha, main_sha, main_sha
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let parent_exists = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/parent"])
        .status()
        .expect("check feat/parent");
    assert!(
        !parent_exists.success(),
        "expected merged parent branch to be deleted locally"
    );

    let child_exists = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/child"])
        .status()
        .expect("check feat/child");
    assert!(
        !child_exists.success(),
        "expected merged child branch to be deleted locally"
    );

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM branches WHERE name != 'main'", [], |row| {
            row.get(0)
        })
        .expect("count branches");
    assert_eq!(
        remaining, 0,
        "expected merged stack metadata branches to be pruned"
    );
}

#[cfg(unix)]
#[test]
fn sync_prunes_metadata_when_merged_branch_ref_is_already_missing() {
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

    run_git(repo.path(), &["branch", "-D", "feat/child"]);

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

    let fake_bin = repo.path().join("fake-bin-prune-missing-ref");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":12,\"state\":\"MERGED\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/12\",\"body\":\"\"}}]}},\"h1\":{{\"nodes\":[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            main_sha, main_sha, main_sha, main_sha
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let parent_exists = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/parent"])
        .status()
        .expect("check feat/parent");
    assert!(
        !parent_exists.success(),
        "expected merged parent branch to be deleted locally"
    );

    let child_exists = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/child"])
        .status()
        .expect("check feat/child");
    assert!(
        !child_exists.success(),
        "expected missing merged child branch ref to remain absent"
    );

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM branches WHERE name != 'main'", [], |row| {
            row.get(0)
        })
        .expect("count branches");
    assert_eq!(
        remaining, 0,
        "expected merged stack metadata branches to be pruned even if a local ref is already missing"
    );
}

#[cfg(unix)]
#[test]
fn sync_does_not_prune_partially_merged_stack() {
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

    let fake_bin = repo.path().join("fake-bin-no-prune-partial");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then\n  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'\n  exit 0\nfi\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":12,\"state\":\"MERGED\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/12\",\"body\":\"\"}}]}},\"h1\":{{\"nodes\":[{{\"number\":11,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"mergeCommit\":null,\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]}}}}}}}}'\n  exit 0\nfi\necho '[]'\n",
            main_sha
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", &test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let child_exists = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/child"])
        .status()
        .expect("check feat/child");
    assert!(
        child_exists.success(),
        "expected merged child branch not to be pruned until full stack is merged"
    );

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    let child_record_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/child'",
            [],
            |row| row.get(0),
        )
        .expect("count child record");
    assert_eq!(
        child_record_count, 1,
        "expected merged child metadata to remain until full stack is merged"
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
            "https://build-user:super-secret@github.com/acme/stack-test.git",
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
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"mergeCommit\":null,\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":null,\"body\":\"Existing reviewer notes\"}}]}},\"h1\":{{\"nodes\":[]}}}}}}}}'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" ]]; then\n  exit 0\nfi\necho '[]'\n",
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
        gh_calls.contains("pr edit 42 --repo acme/stack-test --body"),
        "expected pr edit call for managed body refresh, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("api graphql"),
        "expected batched graphql metadata request, got: {gh_calls}"
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
        gh_calls.contains("https://github.com/acme/stack-test/tree/feat/parent"),
        "expected credential-free managed links, got: {gh_calls}"
    );
    assert!(!gh_calls.contains("super-secret"));
    assert!(!gh_calls.contains("build-user"));
    assert!(
        !gh_calls.contains("/pull/6944"),
        "expected stale cached parent PR not to be reused, got: {gh_calls}"
    );
}

#[test]
fn sync_updates_existing_pr_base_branch_when_it_drifted() {
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

    let fake_bin = repo.path().join("fake-bin-base");
    let gh_log = repo.path().join("gh-base.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/child\",\"mergeCommit\":null,\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\",\"body\":\"Existing reviewer notes\"}}]}},\"h1\":{{\"nodes\":[]}}}}}}}}'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" ]]; then\n  exit 0\nfi\necho '[]'\n",
            gh_log.display()
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

    let gh_calls = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(
        gh_calls.contains("pr edit 42 --repo acme/stack-test --base feat/parent"),
        "expected pr base correction call, got: {gh_calls}"
    );
}

#[test]
fn sync_skips_pr_base_update_when_target_branch_is_in_different_repo() {
    let repo = init_repo_without_origin();
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
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
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();
    run_git(
        repo.path(),
        &["config", "branch.feat/parent.remote", "origin"],
    );
    run_git(repo.path(), &["checkout", "main"]);

    let fake_bin = repo.path().join("fake-bin-cross-repo-base");
    let gh_log = repo.path().join("gh-cross-repo-base.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let real_git = Command::new("sh")
        .args(["-lc", "command -v git"])
        .output()
        .expect("resolve real git");
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout)
        .expect("utf8 git path")
        .trim()
        .to_string();
    let main_sha = git_ref_sha(repo.path(), "refs/heads/main").expect("main sha");
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"ls-remote\" ]]; then\n  printf '{}\\trefs/heads/main\\n'\n  exit 0\nfi\nif [[ \"$1\" == \"fetch\" && \"$2\" == \"--\" && \"$3\" == \"upstream\" ]]; then\n  exec '{}' update-ref refs/remotes/upstream/main '{}'\nfi\nexec '{}' \"$@\"\n",
            main_sha, real_git, main_sha, real_git
        ),
    )
    .expect("write fake git");
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then\n  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/child\",\"mergeCommit\":null,\"headRepositoryOwner\":{{\"login\":\"alice\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\",\"body\":\"Existing reviewer notes\"}}]}},\"h1\":{{\"nodes\":[]}}}}}}}}'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" ]]; then\n  exit 0\nfi\necho '[]'\n",
            gh_log.display()
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
        .success()
        .stderr(predicate::str::contains(
            "skipping PR base update for 'feat/child'",
        ));

    let gh_calls = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(
        !gh_calls.contains("pr edit 42 --base feat/parent"),
        "expected cross-repo base update to be skipped, got: {gh_calls}"
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
