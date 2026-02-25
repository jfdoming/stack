#[test]
fn rename_command_renames_local_branch_and_db_record() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/old"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["--yes", "rename", "feat/old", "feat/new"])
        .assert()
        .success();

    let old_branch = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--list", "feat/old"])
        .output()
        .expect("git branch list old");
    assert!(old_branch.status.success());
    assert!(
        String::from_utf8(old_branch.stdout)
            .expect("utf8 old")
            .trim()
            .is_empty(),
        "feat/old should be renamed away"
    );

    let new_branch = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--list", "feat/new"])
        .output()
        .expect("git branch list new");
    assert!(new_branch.status.success());
    assert!(
        !String::from_utf8(new_branch.stdout)
            .expect("utf8 new")
            .trim()
            .is_empty(),
        "feat/new should exist"
    );

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    let new_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/new'",
            [],
            |row| row.get(0),
        )
        .expect("count new");
    let old_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/old'",
            [],
            |row| row.get(0),
        )
        .expect("count old");

    assert_eq!(new_count, 1);
    assert_eq!(old_count, 0);
}

#[test]
fn rename_preserves_child_parent_relationships_after_parent_rename() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["--yes", "rename", "feat/parent", "feat/parent-renamed"])
        .assert()
        .success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    let parent_name: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/child'",
            [],
            |row| row.get(0),
        )
        .expect("query child parent");
    assert_eq!(parent_name, "feat/parent-renamed");
}

#[test]
fn rename_dry_run_reports_plan_and_makes_no_changes() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/old"])
        .assert()
        .success();

    let output = stack_cmd(repo.path())
        .args(["rename", "feat/old", "feat/new", "--dry-run", "--porcelain"])
        .output()
        .expect("run rename dry-run");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["old_branch"], "feat/old");
    assert_eq!(json["new_branch"], "feat/new");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["applied"], false);

    let old_branch = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--list", "feat/old"])
        .output()
        .expect("git branch list old");
    assert!(
        !String::from_utf8(old_branch.stdout)
            .expect("utf8 old")
            .trim()
            .is_empty(),
        "feat/old should still exist after dry-run"
    );
}

#[cfg(unix)]
#[test]
fn rename_with_open_pr_requires_yes_in_non_interactive_mode() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/open-pr"])
        .assert()
        .success();

    let bare = configure_local_push_url(repo.path());
    run_git(repo.path(), &["push", "--set-upstream", "origin", "feat/open-pr"]);

    let fake_bin = repo.path().join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    std::fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/open-pr\"* ]]; then\n  echo '[{\"number\": 77, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"mergeCommit\": null}]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    #[cfg(unix)]
    std::fs::set_permissions(&fake_gh, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake gh");

    let current_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["rename", "feat/open-pr", "feat/open-pr-renamed"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "requires --yes in non-interactive mode",
        ));

    let old_remote = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/open-pr"])
        .status()
        .expect("verify old remote branch");
    assert!(old_remote.success(), "old remote branch should remain");
}

#[cfg(unix)]
#[test]
fn rename_with_open_pr_and_yes_warns_and_deletes_old_remote_ref() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/open-pr"])
        .assert()
        .success();

    let bare = configure_local_push_url(repo.path());
    run_git(repo.path(), &["push", "--set-upstream", "origin", "feat/open-pr"]);

    let fake_bin = repo.path().join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    std::fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/open-pr\"* ]]; then\n  echo '[{\"number\": 88, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"mergeCommit\": null}]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    #[cfg(unix)]
    std::fs::set_permissions(&fake_gh, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake gh");

    let current_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "rename", "feat/open-pr", "feat/open-pr-renamed"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "deleting the upstream branch may close the open PR",
        ));

    let old_remote = Command::new("git")
        .current_dir(&bare)
        .args(["show-ref", "--verify", "--quiet", "refs/heads/feat/open-pr"])
        .status()
        .expect("verify old remote branch");
    assert!(!old_remote.success(), "old remote branch should be deleted");

    let new_remote = Command::new("git")
        .current_dir(&bare)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/feat/open-pr-renamed",
        ])
        .status()
        .expect("verify new remote branch");
    assert!(new_remote.success(), "new remote branch should exist");
}

#[test]
fn rename_rejects_base_branch() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["rename", "main", "main-renamed"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot rename base branch"));
}

#[test]
fn rename_rejects_when_new_branch_already_exists() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/old"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/new"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["rename", "feat/old", "feat/new"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("new branch already exists in git"));
}

#[test]
fn rename_does_not_push_or_delete_remote_when_source_branch_has_no_upstream() {
    let repo = init_repo();
    let bare = configure_local_push_url(repo.path());

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/local-only"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["--yes", "rename", "feat/local-only", "feat/local-renamed"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "skipped remote update (source branch has no upstream)",
        ));

    let new_remote = Command::new("git")
        .current_dir(&bare)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/feat/local-renamed",
        ])
        .status()
        .expect("verify new remote branch");
    assert!(
        !new_remote.success(),
        "renamed local-only branch should not be pushed"
    );
}
