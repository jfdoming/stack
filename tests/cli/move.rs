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
        .args(["move", "feat/child", "--parent", "feat/parent", "--porcelain"])
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
        .args([
            "move",
            "feat/untracked-target",
            "--parent",
            "feat/parent",
        ])
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
fn move_restores_the_previous_parent_when_sync_execution_fails() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/target"])
        .assert()
        .success();
    fs::write(repo.path().join("target.txt"), "target\n").expect("write target change");
    run_git(repo.path(), &["add", "target.txt"]);
    run_git(repo.path(), &["commit", "-m", "target change"]);

    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/new-parent"])
        .assert()
        .success();
    fs::write(repo.path().join("parent.txt"), "parent\n").expect("write parent change");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent change"]);
    run_git(repo.path(), &["checkout", "main"]);

    let fake_bin = repo.path().join("fake-bin-failing-move-sync");
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
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"replay\" || \"$1\" == \"rebase\" ]]; then\n  echo 'forced move sync failure' >&2\n  exit 1\nfi\nexec '{}' \"$@\"\n",
            real_git
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
        .stderr(predicate::str::contains("sync failed"))
        .stderr(predicate::str::contains("forced move sync failure"));

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
    run_git(
        repo.path(),
        &["branch", "-D", "--", "feat/missing-parent"],
    );

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
