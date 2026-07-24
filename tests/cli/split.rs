fn commit_file(repo: &Path, path: &str, body: &str, message: &str) -> String {
    fs::write(repo.join(path), body).expect("write file");
    run_git(repo, &["add", path]);
    run_git(repo, &["commit", "-m", message]);
    let output = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read head");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn branch_tip(repo: &Path, branch: &str) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", branch])
        .output()
        .expect("read branch tip");
    assert!(
        output.status.success(),
        "rev-parse {branch} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn db_parent(repo: &Path, branch: &str) -> String {
    let db_path = repo.join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    conn.query_row(
        "SELECT p.name
         FROM branches c
         JOIN branches p ON p.id = c.parent_branch_id
         WHERE c.name = ?1",
        [branch],
        |row| row.get(0),
    )
    .expect("branch parent")
}

#[test]
fn split_explicit_at_name_creates_branch_and_tracks_chain() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    let _second = commit_file(repo.path(), "two.txt", "two\n", "two");

    stack_cmd(repo.path())
        .args([
            "split",
            "--at",
            &first,
            "--name",
            "feat/part-1",
            "--yes",
            "--porcelain",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"action\": \"split\""))
        .stdout(predicate::str::contains("\"applied\": true"))
        .stdout(predicate::str::contains("\"current\": \"feat/all\""))
        .stdout(predicate::str::contains("\"parent\": \"main\""))
        .stdout(predicate::str::contains("\"name\": \"feat/part-1\""))
        .stdout(predicate::str::contains("\"top_parent\": \"feat/part-1\""));

    assert_eq!(branch_tip(repo.path(), "feat/part-1"), first);
    assert_eq!(db_parent(repo.path(), "feat/part-1"), "main");
    assert_eq!(db_parent(repo.path(), "feat/all"), "feat/part-1");
    assert_eq!(branch_tip(repo.path(), "feat/all"), branch_tip(repo.path(), "HEAD"));
}

#[test]
fn split_multiple_points_tracks_bottom_to_top_commit_order() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    let second = commit_file(repo.path(), "two.txt", "two\n", "two");
    let _third = commit_file(repo.path(), "three.txt", "three\n", "three");

    stack_cmd(repo.path())
        .args([
            "split",
            "--at",
            &second,
            "--name",
            "feat/part-2",
            "--at",
            &first,
            "--name",
            "feat/part-1",
            "--yes",
        ])
        .assert()
        .success();

    assert_eq!(branch_tip(repo.path(), "feat/part-1"), first);
    assert_eq!(branch_tip(repo.path(), "feat/part-2"), second);
    assert_eq!(db_parent(repo.path(), "feat/part-1"), "main");
    assert_eq!(db_parent(repo.path(), "feat/part-2"), "feat/part-1");
    assert_eq!(db_parent(repo.path(), "feat/all"), "feat/part-2");
}

#[test]
fn split_uses_tracked_parent_when_current_is_tracked() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/parent"]);
    commit_file(repo.path(), "parent.txt", "parent\n", "parent");
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    let _second = commit_file(repo.path(), "two.txt", "two\n", "two");

    stack_cmd(repo.path())
        .args(["track", "feat/parent", "--parent", "main"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["track", "feat/all", "--parent", "feat/parent"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["split", "--at", &first, "--name", "feat/part-1", "--yes"])
        .assert()
        .success();

    assert_eq!(db_parent(repo.path(), "feat/part-1"), "feat/parent");
    assert_eq!(db_parent(repo.path(), "feat/all"), "feat/part-1");
}

#[test]
fn split_without_at_fails_non_interactive_with_semantics() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    commit_file(repo.path(), "one.txt", "one\n", "one");
    commit_file(repo.path(), "two.txt", "two\n", "two");

    stack_cmd(repo.path())
        .args(["split"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "selected commits become tips of new lower stack branches",
        ))
        .stderr(predicate::str::contains(
            "current branch remains the top branch",
        ));
}

#[test]
fn split_rejects_mismatched_at_and_name_counts() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    let second = commit_file(repo.path(), "two.txt", "two\n", "two");
    commit_file(repo.path(), "three.txt", "three\n", "three");

    stack_cmd(repo.path())
        .args(["split", "--at", &first, "--at", &second, "--name", "feat/one"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "each --at commit requires exactly one --name branch",
        ));
}

#[test]
fn split_requires_confirmation_without_yes_in_non_interactive_mode() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    commit_file(repo.path(), "two.txt", "two\n", "two");

    stack_cmd(repo.path())
        .args(["split", "--at", &first, "--name", "feat/part-1"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Planned stack:"))
        .stdout(predicate::str::contains("feat/part-1"))
        .stdout(predicate::str::contains("one"))
        .stdout(predicate::str::contains("feat/all"))
        .stdout(predicate::str::contains("two"))
        .stderr(predicate::str::contains(
            "split requires confirmation; rerun with --yes or --dry-run",
        ));

    let split_ref = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/part-1"])
        .status()
        .expect("check split ref");
    assert!(!split_ref.success(), "unconfirmed split must not create branch");
}

#[test]
fn split_top_name_renames_current_top_branch_and_clears_old_identity() {
    let repo = init_repo_with_named_remote("origin");
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/all"])
        .assert()
        .success();
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    let second = commit_file(repo.path(), "two.txt", "two\n", "two");
    run_git(
        repo.path(),
        &["push", "--set-upstream", "origin", "feat/all"],
    );

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches
         SET cached_pr_number = 42,
             cached_pr_state = 'open',
             cached_pr_head_oid = ?1
         WHERE name = 'feat/all'",
        [&second],
    )
    .expect("seed cached PR identity");
    drop(conn);

    stack_cmd(repo.path())
        .args([
            "split",
            "--at",
            &first,
            "--name",
            "feat/part-1",
            "--top-name",
            "feat/top",
            "--yes",
            "--porcelain",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"old_name\": \"feat/all\""))
        .stdout(predicate::str::contains("\"name\": \"feat/top\""))
        .stdout(predicate::str::contains("\"renamed\": true"));

    assert_eq!(branch_tip(repo.path(), "feat/top"), second);
    assert_eq!(branch_tip(repo.path(), "HEAD"), second);
    assert_eq!(db_parent(repo.path(), "feat/part-1"), "main");
    assert_eq!(db_parent(repo.path(), "feat/top"), "feat/part-1");

    let upstream = Command::new("git")
        .current_dir(repo.path())
        .args([
            "for-each-ref",
            "--format=%(upstream:short)",
            "refs/heads/feat/top",
        ])
        .output()
        .expect("read renamed upstream");
    assert!(upstream.status.success());
    assert!(
        String::from_utf8(upstream.stdout)
            .expect("upstream utf8")
            .trim()
            .is_empty(),
        "renamed top branch must not retain the old branch's upstream"
    );

    let conn = Connection::open(&db_path).expect("reopen db");
    let cached_identity: (Option<i64>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT cached_pr_number, cached_pr_state, cached_pr_head_oid
             FROM branches
             WHERE name = 'feat/top'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read renamed PR cache");
    assert_eq!(cached_identity, (None, None, None));

    run_git(
        repo.path(),
        &[
            "ls-remote",
            "--exit-code",
            "--heads",
            "origin",
            "refs/heads/feat/all",
        ],
    );

    let old_ref = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/all"])
        .status()
        .expect("check old top ref");
    assert!(!old_ref.success(), "expected old current branch name to be renamed");
}

#[test]
fn split_rejects_head_existing_branch_and_merge_commits() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    commit_file(repo.path(), "two.txt", "two\n", "two");
    let head = branch_tip(repo.path(), "HEAD");
    run_git(repo.path(), &["branch", "feat/existing", &first]);

    stack_cmd(repo.path())
        .args(["split", "--at", &head, "--name", "feat/head"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot split at HEAD"));

    stack_cmd(repo.path())
        .args(["split", "--at", &first, "--name", "feat/existing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("branch already exists: feat/existing"));

    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/merge"]);
    let merge_first = commit_file(repo.path(), "merge-one.txt", "one\n", "merge one");
    run_git(repo.path(), &["checkout", "-b", "feat/side", "main"]);
    commit_file(repo.path(), "side.txt", "side\n", "side");
    run_git(repo.path(), &["checkout", "feat/merge"]);
    run_git(repo.path(), &["merge", "--no-ff", "feat/side", "-m", "merge side"]);

    stack_cmd(repo.path())
        .args(["split", "--at", &merge_first, "--name", "feat/merge-part"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "split currently supports linear histories",
        ))
        .stderr(predicate::str::contains("rebase or flatten"));
}

#[test]
fn split_dry_run_porcelain_reports_without_mutating() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    commit_file(repo.path(), "two.txt", "two\n", "two");

    let assert = stack_cmd(repo.path())
        .args([
            "split",
            "--at",
            &first,
            "--name",
            "feat/part-1",
            "--dry-run",
            "--porcelain",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout utf8");
    let payload: Value = serde_json::from_str(&stdout).expect("json payload");
    assert_eq!(payload["action"], "split");
    assert_eq!(payload["applied"], false);
    assert_eq!(payload["splits"][0]["name"], "feat/part-1");
    assert_eq!(payload["top"]["name"], "feat/all");

    let split_ref = Command::new("git")
        .current_dir(repo.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/part-1"])
        .status()
        .expect("check split ref");
    assert!(!split_ref.success(), "dry-run must not create branch ref");

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/part-1'",
            [],
            |row| row.get(0),
        )
        .expect("count branch records");
    assert_eq!(count, 0);
}

#[test]
fn split_treats_a_full_oid_named_parent_as_a_branch() {
    let repo = init_repo_without_origin();
    let original = branch_tip(repo.path(), "main");
    run_git(repo.path(), &["checkout", "-b", "build-parent"]);
    let parent_tip = commit_file(repo.path(), "parent.txt", "parent\n", "parent");
    run_git(repo.path(), &["branch", &original, &parent_tip]);
    let parent_ref = format!("refs/heads/{original}");
    run_git(
        repo.path(),
        &["checkout", "-b", "feat/top", &parent_ref],
    );
    commit_file(repo.path(), "one.txt", "one\n", "one");
    commit_file(repo.path(), "two.txt", "two\n", "two");

    stack_cmd(repo.path())
        .args([
            "split",
            "--parent",
            &original,
            "--at",
            &parent_tip,
            "--name",
            "feat/invalid",
            "--dry-run",
            "--porcelain",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is outside"));
}

#[test]
fn split_dry_run_plain_shows_planned_stack_and_commits() {
    let repo = init_repo_without_origin();
    run_git(repo.path(), &["checkout", "-b", "feat/all"]);
    let first = commit_file(repo.path(), "one.txt", "one\n", "one");
    commit_file(repo.path(), "two.txt", "two\n", "two");

    stack_cmd(repo.path())
        .args([
            "split",
            "--at",
            &first,
            "--name",
            "feat/part-1",
            "--top-name",
            "feat/top",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Planned stack:"))
        .stdout(predicate::str::contains("main"))
        .stdout(predicate::str::contains("feat/part-1"))
        .stdout(predicate::str::contains("one"))
        .stdout(predicate::str::contains("feat/top (renames feat/all)"))
        .stdout(predicate::str::contains("two"))
        .stdout(predicate::str::contains("Commits:").not());
}
