#[test]
fn move_command_reparents_target_and_keeps_descendants_attached() {
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

    stack_cmd(repo.path())
        .args(["move", "feat/b", "--parent", "main", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"action\": \"move\""))
        .stdout(predicate::str::contains("\"target\": \"feat/b\""))
        .stdout(predicate::str::contains("\"new_parent\": \"main\""));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");

    let feat_b_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/b'",
            [],
            |row| row.get(0),
        )
        .expect("feat/b parent");
    assert_eq!(feat_b_parent, "main");

    let feat_c_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/c'",
            [],
            |row| row.get(0),
        )
        .expect("feat/c parent");
    assert_eq!(feat_c_parent, "feat/b");
}

#[test]
fn move_without_target_defaults_to_current_branch() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/other"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/a"]);

    stack_cmd(repo.path())
        .args(["move", "--parent", "feat/other"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "defaulting target branch to current branch 'feat/a'",
        ));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    let feat_a_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/a'",
            [],
            |row| row.get(0),
        )
        .expect("feat/a parent");
    assert_eq!(feat_a_parent, "feat/other");
}

#[test]
fn move_without_target_defaults_to_current_untracked_branch() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/other"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/untracked"]);

    stack_cmd(repo.path())
        .args(["move", "--parent", "feat/other"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "defaulting target branch to current branch 'feat/untracked'",
        ));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    let parent_name: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/untracked'",
            [],
            |row| row.get(0),
        )
        .expect("untracked branch parent");
    assert_eq!(parent_name, "feat/other");
}

#[test]
fn move_tracks_untracked_target_and_parent_when_needed() {
    let repo = init_repo_without_origin();

    run_git(repo.path(), &["checkout", "-b", "feat/parent"]);
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/child"]);

    stack_cmd(repo.path())
        .args([
            "move",
            "feat/child",
            "--parent",
            "feat/parent",
            "--porcelain",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"target\": \"feat/child\""))
        .stdout(predicate::str::contains("\"new_parent\": \"feat/parent\""));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    let child_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/child'",
            [],
            |row| row.get(0),
        )
        .expect("feat/child parent");
    assert_eq!(child_parent, "feat/parent");
}

#[cfg(unix)]
#[test]
fn move_restores_the_previous_parent_and_removes_an_inserted_parent_when_sync_fails() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    run_git(repo.path(), &["branch", "feat/new-parent", "main"]);

    stack_cmd(repo.path())
        .env("PATH", git_only_path(repo.path()))
        .args(["move", "feat/target", "--parent", "feat/new-parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("failed to run gh"));

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let target_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/target'",
            [],
            |row| row.get(0),
        )
        .expect("target parent");
    assert_eq!(target_parent, "main");

    let inserted_parent_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/new-parent'",
            [],
            |row| row.get(0),
        )
        .expect("inserted parent count");
    assert_eq!(inserted_parent_count, 0);
}

#[cfg(unix)]
#[test]
fn move_removes_an_inserted_target_when_sync_fails() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    run_git(repo.path(), &["branch", "feat/untracked-target", "main"]);

    stack_cmd(repo.path())
        .env("PATH", git_only_path(repo.path()))
        .args(["move", "feat/untracked-target", "--parent", "feat/parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("failed to run gh"));

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let inserted_target_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/untracked-target'",
            [],
            |row| row.get(0),
        )
        .expect("inserted target count");
    assert_eq!(inserted_target_count, 0);

    let parent_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/parent'",
            [],
            |row| row.get(0),
        )
        .expect("parent branch parent");
    assert_eq!(parent_parent, "main");
}

#[cfg(unix)]
#[test]
fn move_planning_does_not_block_inserted_target_rollback_with_pr_cache_writes() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    run_git(repo.path(), &["branch", "feat/untracked-target", "main"]);
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
    let target_head = git_ref_sha(repo.path(), "refs/heads/feat/untracked-target")
        .expect("untracked target head");

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute_batch(
        "CREATE TRIGGER fail_inserted_target_sync_sha
         BEFORE UPDATE OF last_synced_head_sha ON branches
         WHEN OLD.name = 'feat/untracked-target'
         BEGIN
             SELECT RAISE(ABORT, 'forced inserted target sync failure');
         END;",
    )
    .expect("install sync failure");
    drop(conn);

    let fake_bin = repo.path().join("fake-bin-inserted-target-pr");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash
if [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then
  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'
  exit 0
fi
if [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then
  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[]}},\"h1\":{{\"nodes\":[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/untracked-target\",\"headRefOid\":\"{}\",\"mergeCommit\":null,\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\",\"body\":\"Existing notes\"}}]}}}}}}}}'
  exit 0
fi
echo '[]'
",
            target_head
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
        .args(["move", "feat/untracked-target", "--parent", "feat/parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "forced inserted target sync failure",
        ))
        .stderr(predicate::str::contains("gained sync or PR metadata").not());

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let inserted_target_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/untracked-target'",
            [],
            |row| row.get(0),
        )
        .expect("inserted target count");
    assert_eq!(inserted_target_count, 0);
}

#[cfg(unix)]
#[test]
fn move_retains_applied_local_state_when_deferred_pr_refresh_fails() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/new-parent"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);
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

    let original_target =
        git_ref_sha(repo.path(), "refs/heads/feat/target").expect("original target head");
    let fake_bin = repo.path().join("fake-bin-move-pr-refresh");
    let gh_log = repo.path().join("move-pr-refresh.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash
echo \"$@\" >> '{}'
if [[ \"$1\" == \"repo\" && \"$2\" == \"view\" ]]; then
  echo '{{\"nameWithOwner\":\"acme/stack-test\"}}'
  exit 0
fi
if [[ \"$1\" == \"api\" && \"$2\" == \"graphql\" ]]; then
  echo '{{\"data\":{{\"repository\":{{\"h0\":{{\"nodes\":[]}},\"h1\":{{\"nodes\":[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/target\",\"headRefOid\":\"{}\",\"mergeCommit\":null,\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\",\"body\":\"Existing notes\"}}]}}}}}}}}'
  exit 0
fi
if [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" && \"$*\" == *\" --base \"* ]]; then
  exit 0
fi
if [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" && \"$*\" == *\" --body \"* ]]; then
  echo 'forced PR body refresh failure' >&2
  exit 1
fi
echo '[]'
",
            gh_log.display(),
            original_target
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
        .args(["move", "feat/target", "--parent", "feat/new-parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("move was applied locally"))
        .stderr(predicate::str::contains(
            "forced PR body refresh failure",
        ))
        .stderr(predicate::str::contains("run `stack sync`"));

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let target_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/target'",
            [],
            |row| row.get(0),
        )
        .expect("target parent");
    assert_eq!(target_parent, "feat/new-parent");
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/target").as_deref(),
        Some(original_target.as_str())
    );
    let gh_calls = fs::read_to_string(gh_log).expect("read gh log");
    assert!(gh_calls.contains("pr edit 42 --repo acme/stack-test --base feat/new-parent"));
    assert!(gh_calls.contains("pr edit 42 --repo acme/stack-test --body"));
}

#[cfg(unix)]
#[test]
fn move_preserves_a_concurrent_database_parent_aba_when_topology_rollback_is_stale() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    fs::write(repo.path().join("target.txt"), "target\n").expect("write target change");
    run_git(repo.path(), &["add", "target.txt"]);
    run_git(repo.path(), &["commit", "-m", "target change"]);

    run_git(repo.path(), &["checkout", "main"]);
    for (branch, file) in [
        ("feat/new-parent", "new-parent.txt"),
        ("feat/concurrent-parent", "concurrent-parent.txt"),
    ] {
        stack_cmd(repo.path())
            .args(["create", "--parent", "main", "--name", branch])
            .assert()
            .success();
        fs::write(repo.path().join(file), format!("{branch}\n")).expect("write parent change");
        run_git(repo.path(), &["add", file]);
        run_git(repo.path(), &["commit", "-m", &format!("{branch} change")]);
        run_git(repo.path(), &["checkout", "main"]);
    }

    let original_target =
        git_ref_sha(repo.path(), "refs/heads/feat/target").expect("original target head");
    let db_path = repo.path().join(".git/stack.db");
    let conn = Connection::open(&db_path).expect("open stack db");
    conn.execute_batch(
        "CREATE TRIGGER fail_concurrent_topology_move_sync
         BEFORE UPDATE OF last_synced_head_sha ON branches
         BEGIN
             SELECT RAISE(ABORT, 'forced concurrent topology sync failure');
         END;",
    )
    .expect("install late sync failure");
    drop(conn);

    let fake_bin = repo.path().join("fake-bin-concurrent-move-topology");
    let reparent_ready = fake_bin.join("reparent-ready");
    let reparent_done = fake_bin.join("reparent-done");
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
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        "#!/usr/bin/env bash
set -euo pipefail

if [[ ! -f \"$STACK_TEST_REPARENT_READY\" ]]; then
  target_now=\"$(\"$STACK_TEST_REAL_GIT\" rev-parse refs/heads/feat/target)\"
  if [[ \"$target_now\" != \"$STACK_TEST_ORIGINAL_TARGET\" ]]; then
    printf 'ready\n' > \"$STACK_TEST_REPARENT_READY\"
    for ((attempt = 0; attempt < 1000; attempt++)); do
      [[ -f \"$STACK_TEST_REPARENT_DONE\" ]] && break
      sleep 0.01
    done
    if [[ ! -f \"$STACK_TEST_REPARENT_DONE\" ]]; then
      echo 'timed out waiting for concurrent topology update' >&2
      exit 91
    fi
  fi
fi

exec \"$STACK_TEST_REAL_GIT\" \"$@\"
",
    )
    .expect("write fake git");
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");
    let fake_gh = fake_bin.join("gh");
    fs::write(&fake_gh, "#!/usr/bin/env bash\nexit 1\n").expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    let writer_db_path = db_path.clone();
    let writer_ready = reparent_ready.clone();
    let writer_done = reparent_done.clone();
    let concurrent_writer = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while !writer_ready.exists() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "move did not reach the concurrent topology injection point"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let conn = Connection::open(writer_db_path).expect("open concurrent stack db");
        let updated = conn
            .execute(
                "UPDATE branches
                 SET parent_branch_id = (
                     SELECT id FROM branches WHERE name = 'feat/concurrent-parent'
                 )
                 WHERE name = 'feat/target'",
                [],
            )
            .expect("concurrently reparent target");
        assert_eq!(updated, 1);
        let restored = conn
            .execute(
                "UPDATE branches
                 SET parent_branch_id = (
                     SELECT id FROM branches WHERE name = 'feat/new-parent'
                 )
                 WHERE name = 'feat/target'",
                [],
            )
            .expect("restore target to attempted parent");
        assert_eq!(restored, 1);
        fs::write(writer_done, "done\n").expect("signal concurrent reparent");
    });

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .env("STACK_TEST_REAL_GIT", &real_git)
        .env("STACK_TEST_ORIGINAL_TARGET", &original_target)
        .env("STACK_TEST_REPARENT_READY", &reparent_ready)
        .env("STACK_TEST_REPARENT_DONE", &reparent_done)
        .args(["move", "feat/target", "--parent", "feat/new-parent"])
        .output()
        .expect("run move");
    concurrent_writer.join().expect("concurrent writer");

    assert!(!output.status.success(), "move unexpectedly succeeded");
    let stderr = String::from_utf8(output.stderr).expect("utf8 move stderr");
    assert!(stderr.contains("forced concurrent topology sync failure"));
    assert!(
        stderr.contains("additionally failed to restore move database topology"),
        "missing topology rollback failure: {stderr}"
    );
    assert!(
        stderr.contains("topology changed"),
        "missing stale topology explanation: {stderr}"
    );

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/target").as_deref(),
        Some(original_target.as_str()),
        "Git rollback should still restore the target ref"
    );
    let conn = Connection::open(db_path).expect("open stack db");
    let target_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/target'",
            [],
            |row| row.get(0),
        )
        .expect("target parent");
    assert_eq!(
        target_parent, "feat/new-parent",
        "stale move rollback must preserve the concurrent writer's final topology"
    );
}

#[cfg(unix)]
#[test]
fn move_restores_all_restacked_refs_and_checkout_when_late_sync_persistence_fails() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    fs::write(repo.path().join("target.txt"), "target\n").expect("write target change");
    run_git(repo.path(), &["add", "target.txt"]);
    run_git(repo.path(), &["commit", "-m", "target change"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/target", "--name", "feat/child"])
        .assert()
        .success();
    fs::write(repo.path().join("child.txt"), "child\n").expect("write child change");
    run_git(repo.path(), &["add", "child.txt"]);
    run_git(repo.path(), &["commit", "-m", "child change"]);

    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/new-parent"])
        .assert()
        .success();
    fs::write(repo.path().join("parent.txt"), "parent\n").expect("write parent change");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent change"]);
    run_git(repo.path(), &["checkout", "feat/target"]);

    let original_main = git_ref_sha(repo.path(), "refs/heads/main").expect("original main head");
    let original_target =
        git_ref_sha(repo.path(), "refs/heads/feat/target").expect("original target head");
    let original_child =
        git_ref_sha(repo.path(), "refs/heads/feat/child").expect("original child head");
    let original_parent =
        git_ref_sha(repo.path(), "refs/heads/feat/new-parent").expect("original new parent head");

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute_batch(
        "CREATE TRIGGER fail_late_move_sync
         BEFORE UPDATE OF last_synced_head_sha ON branches
         BEGIN
             SELECT RAISE(ABORT, 'forced late move sync failure');
         END;",
    )
    .expect("install late sync failure");
    drop(conn);

    let fake_bin = repo.path().join("fake-bin-failing-move-sync");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(&fake_gh, "#!/usr/bin/env bash\nexit 1\n").expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["move", "feat/target", "--parent", "feat/new-parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("sync failed"))
        .stderr(predicate::str::contains("forced late move sync failure"));

    for (reference, expected) in [
        ("refs/heads/main", original_main.as_str()),
        ("refs/heads/feat/target", original_target.as_str()),
        ("refs/heads/feat/child", original_child.as_str()),
        ("refs/heads/feat/new-parent", original_parent.as_str()),
    ] {
        assert_eq!(
            git_ref_sha(repo.path(), reference).as_deref(),
            Some(expected),
            "{reference} must be restored exactly"
        );
    }

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    for (branch, expected_parent) in [
        ("feat/target", "main"),
        ("feat/child", "feat/target"),
        ("feat/new-parent", "main"),
    ] {
        let actual_parent: String = conn
            .query_row(
                "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = ?1",
                [branch],
                |row| row.get(0),
            )
            .expect("branch parent");
        assert_eq!(actual_parent, expected_parent);
    }

    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(current.status.success());
    assert_eq!(
        String::from_utf8(current.stdout)
            .expect("utf8 current branch")
            .trim(),
        "feat/target"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "HEAD").as_deref(),
        Some(original_target.as_str())
    );
    let status = Command::new("git")
        .current_dir(repo.path())
        .args(["status", "--porcelain"])
        .output()
        .expect("read worktree status");
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "worktree must match the restored target: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[cfg(unix)]
#[test]
fn move_aborts_its_fallback_rebase_before_restoring_topology() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    fs::write(repo.path().join("README.md"), "target version\n").expect("write target change");
    run_git(repo.path(), &["add", "README.md"]);
    run_git(repo.path(), &["commit", "-m", "target change"]);
    let original_target =
        git_ref_sha(repo.path(), "refs/heads/feat/target").expect("original target head");

    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/new-parent"])
        .assert()
        .success();
    fs::write(repo.path().join("README.md"), "parent version\n").expect("write parent change");
    run_git(repo.path(), &["add", "README.md"]);
    run_git(repo.path(), &["commit", "-m", "parent change"]);
    run_git(repo.path(), &["checkout", "feat/target"]);

    let fake_bin = repo.path().join("fake-bin-move-rebase-conflict");
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
    let fake_gh = fake_bin.join("gh");
    fs::write(&fake_gh, "#!/usr/bin/env bash\nexit 1\n").expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["move", "feat/target", "--parent", "feat/new-parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "sync stopped due to merge conflicts while restacking 'feat/target'",
        ));

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/target").as_deref(),
        Some(original_target.as_str())
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let target_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/target'",
            [],
            |row| row.get(0),
        )
        .expect("target parent");
    assert_eq!(target_parent, "main");

    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(current.status.success());
    assert_eq!(
        String::from_utf8(current.stdout)
            .expect("utf8 current branch")
            .trim(),
        "feat/target"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "HEAD").as_deref(),
        Some(original_target.as_str())
    );
    let status = Command::new("git")
        .current_dir(repo.path())
        .args(["status", "--porcelain"])
        .output()
        .expect("read worktree status");
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "move-owned conflict state must be removed: {}",
        String::from_utf8_lossy(&status.stdout)
    );

    for namespace in ["refs/heads/stack/restack", "refs/stack/restacks"] {
        let refs = Command::new("git")
            .current_dir(repo.path())
            .args(["for-each-ref", "--format=%(refname)", namespace])
            .output()
            .expect("list move recovery refs");
        assert!(refs.status.success());
        assert!(
            refs.stdout.is_empty(),
            "move recovery refs must be removed: {}",
            String::from_utf8_lossy(&refs.stdout)
        );
    }
}

#[cfg(unix)]
#[test]
fn move_retains_proposed_topology_when_atomic_ref_rollback_detects_a_concurrent_advance() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    fs::write(repo.path().join("target.txt"), "target\n").expect("write target change");
    run_git(repo.path(), &["add", "target.txt"]);
    run_git(repo.path(), &["commit", "-m", "target change"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/target", "--name", "feat/child"])
        .assert()
        .success();
    fs::write(repo.path().join("child.txt"), "child\n").expect("write child change");
    run_git(repo.path(), &["add", "child.txt"]);
    run_git(repo.path(), &["commit", "-m", "child change"]);

    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/new-parent"])
        .assert()
        .success();
    fs::write(repo.path().join("parent.txt"), "parent\n").expect("write parent change");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent change"]);
    run_git(repo.path(), &["checkout", "feat/target"]);

    let original_main = git_ref_sha(repo.path(), "refs/heads/main").expect("original main head");
    let original_target =
        git_ref_sha(repo.path(), "refs/heads/feat/target").expect("original target head");
    let original_child =
        git_ref_sha(repo.path(), "refs/heads/feat/child").expect("original child head");
    let original_parent =
        git_ref_sha(repo.path(), "refs/heads/feat/new-parent").expect("original new parent head");

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute_batch(
        "CREATE TRIGGER fail_concurrent_move_sync
         BEFORE UPDATE OF last_synced_head_sha ON branches
         BEGIN
             SELECT RAISE(ABORT, 'forced concurrent move sync failure');
         END;",
    )
    .expect("install late sync failure");
    drop(conn);

    let fake_bin = repo.path().join("fake-bin-concurrent-move-rollback");
    let advance_record = fake_bin.join("advance-record");
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

    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        "#!/usr/bin/env bash
set -euo pipefail

if [[ ! -f \"$STACK_TEST_ADVANCE_RECORD\" ]]; then
  target_now=\"$(\"$STACK_TEST_REAL_GIT\" rev-parse refs/heads/feat/target)\"
  child_now=\"$(\"$STACK_TEST_REAL_GIT\" rev-parse refs/heads/feat/child)\"
  if [[ \"$target_now\" != \"$STACK_TEST_ORIGINAL_TARGET\" &&
        \"$child_now\" != \"$STACK_TEST_ORIGINAL_CHILD\" ]]; then
    concurrent_target=\"$(\"$STACK_TEST_REAL_GIT\" commit-tree \"${target_now}^{tree}\" \
      -p \"$target_now\" -m 'concurrent target advance')\"
    \"$STACK_TEST_REAL_GIT\" update-ref refs/heads/feat/target \
      \"$concurrent_target\" \"$target_now\"
    printf '%s\n%s\n%s\n' \
      \"$concurrent_target\" \"$target_now\" \"$child_now\" \
      > \"$STACK_TEST_ADVANCE_RECORD\"
  fi
fi

exec \"$STACK_TEST_REAL_GIT\" \"$@\"
",
    )
    .expect("write fake git");
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");
    let fake_gh = fake_bin.join("gh");
    fs::write(&fake_gh, "#!/usr/bin/env bash\nexit 1\n").expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env("STACK_TEST_REAL_GIT", &real_git)
        .env("STACK_TEST_ORIGINAL_TARGET", &original_target)
        .env("STACK_TEST_ORIGINAL_CHILD", &original_child)
        .env("STACK_TEST_ADVANCE_RECORD", &advance_record)
        .args(["move", "feat/target", "--parent", "feat/new-parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "forced concurrent move sync failure",
        ))
        .stderr(predicate::str::contains(
            "additionally failed to restore sync-updated local refs",
        ))
        .stderr(predicate::str::contains(
            "could not atomically restore local branch refs",
        ))
        .stderr(predicate::str::contains(
            "retained the proposed move database topology",
        ));

    let advance = fs::read_to_string(&advance_record).expect("read concurrent advance record");
    let mut advance_lines = advance.lines();
    let concurrent_target = advance_lines.next().expect("concurrent target head");
    let restacked_target = advance_lines.next().expect("restacked target head");
    let restacked_child = advance_lines.next().expect("restacked child head");
    assert!(
        advance_lines.next().is_none(),
        "unexpected concurrent advance record: {advance}"
    );
    assert_ne!(restacked_target, original_target);
    assert_ne!(restacked_child, original_child);

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/target").as_deref(),
        Some(concurrent_target),
        "the third-party target advance must be preserved"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/child").as_deref(),
        Some(restacked_child),
        "an atomic rollback failure must not partially restore the child"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/main").as_deref(),
        Some(original_main.as_str())
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/new-parent").as_deref(),
        Some(original_parent.as_str())
    );

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    for (branch, expected_parent) in [
        ("feat/target", "feat/new-parent"),
        ("feat/child", "feat/target"),
        ("feat/new-parent", "main"),
    ] {
        let actual_parent: String = conn
            .query_row(
                "SELECT p.name
                 FROM branches c
                 JOIN branches p ON p.id = c.parent_branch_id
                 WHERE c.name = ?1",
                [branch],
                |row| row.get(0),
            )
            .expect("branch parent");
        assert_eq!(
            actual_parent, expected_parent,
            "the proposed topology must be retained for {branch}"
        );
    }

    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(current.status.success());
    assert_eq!(
        String::from_utf8(current.stdout)
            .expect("utf8 current branch")
            .trim(),
        "feat/target"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "HEAD").as_deref(),
        Some(concurrent_target)
    );
    let status = Command::new("git")
        .current_dir(repo.path())
        .args(["status", "--porcelain"])
        .output()
        .expect("read worktree status");
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "worktree must match the preserved concurrent target: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[test]
fn move_rejects_a_tracked_parent_whose_git_ref_is_missing() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "main",
            "--name",
            "feat/missing-parent",
        ])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    run_git(repo.path(), &["branch", "-D", "--", "feat/missing-parent"]);

    stack_cmd(repo.path())
        .args(["move", "feat/target", "--parent", "feat/missing-parent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "parent branch does not exist in git: feat/missing-parent",
        ));

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let target_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/target'",
            [],
            |row| row.get(0),
        )
        .expect("target parent");
    assert_eq!(target_parent, "main");
}

#[test]
fn move_restacks_target_onto_new_parent_immediately() {
    let repo = init_repo_without_origin();

    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);

    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    fs::write(repo.path().join("b.txt"), "b\n").expect("write b");
    run_git(repo.path(), &["add", "b.txt"]);
    run_git(repo.path(), &["commit", "-m", "b"]);

    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["move", "feat/a", "--parent", "feat/b"])
        .assert()
        .success();

    let parent_is_ancestor = Command::new("git")
        .current_dir(repo.path())
        .args(["merge-base", "--is-ancestor", "feat/b", "feat/a"])
        .status()
        .expect("check feat/b ancestor feat/a");
    assert!(
        parent_is_ancestor.success(),
        "expected feat/a to be restacked onto feat/b after move"
    );
}

#[test]
fn move_requires_parent_in_non_interactive_mode_when_omitted() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["move", "feat/a"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "missing parent branch; usage: stack move [target] --parent <parent>",
        ));
}

#[test]
fn move_rejects_reparenting_onto_descendant() {
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
        .args(["move", "feat/a", "--parent", "feat/b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("link would create a cycle"));
}

#[test]
fn move_rejects_reparenting_onto_nested_descendant() {
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
        .args(["move", "feat/a", "--parent", "feat/c"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("link would create a cycle"));
}
