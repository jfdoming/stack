#[test]
fn pr_push_treats_a_plus_prefixed_branch_as_a_literal_ref() {
    let repo = init_repo();
    let bare = configure_local_push_url(repo.path());

    run_git(repo.path(), &["checkout", "-b", "victim"]);
    fs::write(repo.path().join("victim.txt"), "victim\n").expect("write victim file");
    run_git(repo.path(), &["add", "victim.txt"]);
    run_git(repo.path(), &["commit", "-m", "victim"]);
    run_git(repo.path(), &["push", "origin", "victim"]);

    run_git(repo.path(), &["checkout", "-b", "remote-advance"]);
    fs::write(repo.path().join("remote.txt"), "remote advance\n")
        .expect("write remote advancement");
    run_git(repo.path(), &["add", "remote.txt"]);
    run_git(repo.path(), &["commit", "-m", "remote advance"]);
    run_git(
        repo.path(),
        &[
            "push",
            "origin",
            "refs/heads/remote-advance:refs/heads/victim",
        ],
    );
    let remote_victim_before = git_ref_sha(&bare, "refs/heads/victim");

    run_git(repo.path(), &["checkout", "main"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "+victim"])
        .assert()
        .success();
    fs::write(repo.path().join("literal.txt"), "literal\n").expect("write literal file");
    run_git(repo.path(), &["add", "literal.txt"]);
    run_git(repo.path(), &["commit", "-m", "literal"]);
    let literal_sha = git_ref_sha(repo.path(), "refs/heads/+victim");

    stack_cmd(repo.path()).arg("pr").assert().success();

    assert_eq!(
        git_ref_sha(&bare, "refs/heads/victim"),
        remote_victim_before
    );
    assert_eq!(git_ref_sha(&bare, "refs/heads/+victim"), literal_sha);
}

#[test]
fn pr_dry_run_uses_parent_branch_as_base() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/child"]);

    let output = stack_cmd(repo.path())
        .args(["pr", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack pr --dry-run");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["head"], "feat/child");
    assert_eq!(json["base"], "feat/parent");

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let stored: Option<String> = conn
        .query_row("SELECT push_target FROM repo_meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("read push target");
    assert!(stored.is_none(), "dry run must not persist first-push config");
}
#[test]
fn pr_dry_run_on_untracked_branch_warns_and_uses_base() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/untracked"]);

    let output = stack_cmd(repo.path())
        .args(["pr", "--dry-run"])
        .output()
        .expect("run stack pr --dry-run");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is not stacked"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("would push '"));
}
#[test]
fn pr_dry_run_on_parentless_tracked_branch_warns_and_uses_base() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/orphan"]);
    stack_cmd(repo.path()).assert().success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    conn.execute(
        "INSERT INTO branches(name, parent_branch_id) VALUES ('feat/orphan', NULL)",
        [],
    )
    .expect("insert orphan tracked branch");

    let output = stack_cmd(repo.path())
        .args(["pr", "--dry-run"])
        .output()
        .expect("run stack pr --dry-run");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is not stacked"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("would push '"));
}

#[cfg(unix)]#[test]
fn pr_does_not_create_when_existing_pr_is_found() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/existing"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/existing"]);

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/existing\"* ]]; then\n  echo '[{\"number\": 77, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"headRefName\": \"feat/existing\", \"headRepositoryOwner\": {\"login\": \"acme\"}, \"mergeCommit\": null}]'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'create should not be called' >&2\n  exit 1\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "PR already exists for 'feat/existing': #77",
        ));
}

#[cfg(unix)]#[test]
fn pr_existing_lookup_handles_coloured_gh_json_output() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "main",
            "--name",
            "feat/existing-colour",
        ])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/existing-colour"]);

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/existing-colour\"* ]]; then\n  printf '\\033[32m[\\n  {\\n    \"number\": 77,\\n    \"state\": \"OPEN\",\\n    \"baseRefName\": \"main\",\\n    \"headRefName\": \"feat/existing-colour\",\\n    \"headRepositoryOwner\": {\"login\": \"acme\"},\\n    \"mergeCommit\": null\\n  }\\n]\\033[0m\\n'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'create should not be called' >&2\n  exit 1\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "PR already exists for 'feat/existing-colour': #77",
        ));
}

#[cfg(unix)]#[test]
fn pr_porcelain_reports_existing_pr_without_create() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/existing-json"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/existing-json"]);

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/existing-json\"* ]]; then\n  echo '[{\"number\": 88, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"headRefName\": \"feat/existing-json\", \"headRepositoryOwner\": {\"login\": \"acme\"}, \"mergeCommit\": null}]'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'create should not be called' >&2\n  exit 1\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["pr", "--porcelain"])
        .output()
        .expect("run stack pr porcelain");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["existing_pr_number"], 88);
    assert_eq!(json["will_open_link"], false);
}

#[cfg(unix)]#[test]
fn pr_yes_pushes_and_prints_pr_open_link() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-create"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-create"]);
    let bare = configure_local_push_url(repo.path());

    let fake_bin = repo.path().join("fake-bin");
    let open_log = repo.path().join("open.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/pr-create\"* ]]; then\n  echo '[]'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'https://github.com/acme/stack-test/pull/99'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "pushed 'feat/pr-create' to 'origin'",
        ))
        .stdout(predicate::str::contains("opened PR URL in browser"));

    let open_calls = fs::read_to_string(&open_log).expect("read open log");
    assert!(
        open_calls
            .contains("https://github.com/acme/stack-test/compare/main...feat/pr-create?expand=1"),
        "expected browser opener call, got: {open_calls}"
    );

    let pushed = Command::new("git")
        .current_dir(&bare)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/feat/pr-create",
        ])
        .status()
        .expect("verify pushed branch");
    assert!(
        pushed.success(),
        "expected pushed branch to exist on bare remote"
    );
}

#[cfg(unix)]
#[test]
fn pr_browser_url_redacts_remote_credentials() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "https://build-user:super-secret@github.com/acme/stack-test.git",
        ],
    );
    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "main",
            "--name",
            "feat/credential-browser",
        ])
        .assert()
        .success();
    configure_local_push_url(repo.path());

    let fake_bin = repo.path().join("fake-bin-credential-browser");
    let open_log = repo.path().join("credential-browser.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(&fake_gh, "#!/usr/bin/env bash\necho '[]'\n").expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success();

    let open_calls = fs::read_to_string(&open_log).expect("read browser log");
    assert!(
        open_calls.contains(
            "https://github.com/acme/stack-test/compare/main...feat/credential-browser"
        ),
        "expected a credential-free GitHub URL, got: {open_calls}"
    );
    assert!(!open_calls.contains("super-secret"));
    assert!(!open_calls.contains("build-user"));
}

#[cfg(unix)]#[test]
fn pr_uses_upstream_compare_url_when_branch_remote_is_fork() {
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

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/fork-pr-open"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/fork-pr-open"]);
    run_git(
        repo.path(),
        &["config", "branch.feat/fork-pr-open.remote", "origin"],
    );
    configure_local_push_url(repo.path());

    let fake_bin = repo.path().join("fake-bin");
    let open_log = repo.path().join("open.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/fork-pr-open\"* ]]; then\n  echo '[]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains("opened PR URL in browser"));

    let open_calls = fs::read_to_string(&open_log).expect("read open log");
    assert!(
        open_calls.contains(
            "https://github.com/acme/stack-test/compare/main...alice:feat/fork-pr-open?expand=1"
        ),
        "expected browser opener call, got: {open_calls}"
    );
}

#[cfg(unix)]#[test]
fn pr_uses_upstream_compare_url_when_head_branch_remote_is_upstream() {
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

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/upstream-head"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/upstream-head"]);
    run_git(
        repo.path(),
        &["config", "branch.feat/upstream-head.remote", "upstream"],
    );
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

    let fake_bin = repo.path().join("fake-bin");
    let open_log = repo.path().join("open.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/upstream-head\"* ]]; then\n  echo '[]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains("opened PR URL in browser"));

    let open_calls = fs::read_to_string(&open_log).expect("read open log");
    assert!(
        open_calls.contains(
            "https://github.com/acme/stack-test/compare/main...feat/upstream-head?expand=1"
        ),
        "expected upstream compare URL, got: {open_calls}"
    );
}

#[cfg(unix)]#[test]
fn pr_uses_configured_upstream_target_for_new_branch() {
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
        .args(["create", "--parent", "main", "--name", "feat/upstream-default"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/upstream-default"]);

    let fake_bin = repo.path().join("fake-bin");
    let open_log = repo.path().join("open.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/upstream-default\"* ]]; then\n  echo '[]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "pushed 'feat/upstream-default' to 'upstream'",
        ));

    let open_calls = fs::read_to_string(&open_log).expect("read open log");
    assert!(
        open_calls.contains(
            "https://github.com/acme/stack-test/compare/main...feat/upstream-default?expand=1"
        ),
        "expected upstream compare URL, got: {open_calls}"
    );
}

#[cfg(unix)]#[test]
fn pr_url_includes_managed_section_links_for_stacked_branch() {
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
        .args([
            "create",
            "--parent",
            "feat/child",
            "--name",
            "feat/grandchild",
        ])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/child"]);
    configure_local_push_url(repo.path());

    let fake_bin = repo.path().join("fake-bin");
    let open_log = repo.path().join("open.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/child\"* ]]; then\n  echo '[]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success();

    let open_calls = fs::read_to_string(&open_log).expect("read open log");
    assert!(
        open_calls.contains("body=%3C%21--"),
        "expected encoded managed body comment marker, got: {open_calls}"
    );
    assert!(
        open_calls.contains("stack%3Amanaged%3Astart"),
        "expected managed start marker in body, got: {open_calls}"
    );
    assert!(
        open_calls.contains("stack%3Amanaged%3Aend"),
        "expected managed end marker in body, got: {open_calls}"
    );
    assert!(
        open_calls.contains("%E2%86%92"),
        "expected unicode arrow in managed body, got: {open_calls}"
    );
    assert!(
        open_calls.contains("feat%2Fparent"),
        "expected parent branch link in body, got: {open_calls}"
    );
    assert!(
        open_calls.contains("feat%2Fgrandchild"),
        "expected child branch link in body, got: {open_calls}"
    );
}

#[cfg(unix)]#[test]
fn pr_url_uses_base_commit_link_for_base_parent() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/base-commit-link"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/base-commit-link"]);
    configure_local_push_url(repo.path());

    let base_sha = String::from_utf8(
        Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "main"])
            .output()
            .expect("read main sha")
            .stdout,
    )
    .expect("main sha utf8")
    .trim()
    .to_string();

    let fake_bin = repo.path().join("fake-bin");
    let open_log = repo.path().join("open.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/base-commit-link\"* ]]; then\n  echo '[]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success();

    let open_calls = fs::read_to_string(&open_log).expect("read open log");
    assert!(
        open_calls.contains(&format!("%2Fcommit%2F{base_sha}")),
        "expected base branch commit link in body, got: {open_calls}"
    );
}

#[cfg(unix)]#[test]
fn pr_handles_existing_lookup_parse_failure_with_friendly_warning() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-parse"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-parse"]);
    configure_local_push_url(repo.path());

    let fake_bin = repo.path().join("fake-bin");
    let open_log = repo.path().join("open.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    install_fake_browser_openers(&fake_bin, &open_log);
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/pr-parse\"* ]]; then\n  echo '<html>bad gateway</html>'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'https://github.com/acme/stack-test/pull/100'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .env_remove("STACK_MOCK_BROWSER_OPEN")
        .args(["--yes", "pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains("opened PR URL in browser"))
        .stderr(predicate::str::contains(
            "could not determine existing PR status from gh",
        ));

    let open_calls = fs::read_to_string(&open_log).expect("read open log");
    assert!(
        open_calls
            .contains("https://github.com/acme/stack-test/compare/main...feat/pr-parse?expand=1"),
        "expected browser opener call, got: {open_calls}"
    );
}

#[cfg(unix)]#[test]
fn pr_debug_flag_prints_full_gh_parse_error_details() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-debug"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-debug"]);
    configure_local_push_url(repo.path());

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/pr-debug\"* ]]; then\n  echo '<html>bad gateway</html>'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'https://github.com/acme/stack-test/pull/101'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "--debug", "pr"])
        .assert()
        .success()
        .stderr(predicate::str::contains("failed to parse gh PR list JSON"))
        .stderr(predicate::str::contains("<html>bad gateway</html>"));
}

#[cfg(unix)]#[test]
fn pr_open_fails_when_push_remote_is_missing() {
    let repo = init_repo_without_origin();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-fail"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-fail"]);

    stack_cmd(repo.path())
        .args(["--yes", "pr"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no Git remote pushes to the canonical repository",
        ));
}

#[cfg(unix)]
#[test]
fn pr_blocks_child_when_stack_is_placed_only_in_fork() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &["remote", "set-url", "origin", "git@github.com:alice/stack-test.git"],
    );
    run_git(
        repo.path(),
        &["remote", "add", "upstream", "git@github.com:acme/stack-test.git"],
    );
    stack_cmd(repo.path())
        .args(["config", "push-target", "fork"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(&fake_gh, "#!/usr/bin/env bash\necho '[]'\n").expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!("{}:{}", fake_bin.display(), env::var("PATH").unwrap_or_default());

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["pr"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "parent 'feat/parent' exists only in the fork",
        ));
}
#[test]
fn pr_opens_in_non_interactive_mode_without_yes() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-confirm"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-confirm"]);
    configure_local_push_url(repo.path());

    stack_cmd(repo.path())
        .args(["pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pushed 'feat/pr-confirm'"))
        .stdout(predicate::str::contains("opened PR URL in browser"));
}
#[test]
fn pr_untracked_branch_opens_in_non_interactive_mode_without_yes() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/nonstacked-pr"]);
    configure_local_push_url(repo.path());

    stack_cmd(repo.path())
        .args(["pr"])
        .assert()
        .success()
        .stderr(predicate::str::contains("is not stacked"))
        .stdout(predicate::str::contains("pushed 'feat/nonstacked-pr'"))
        .stdout(predicate::str::contains("opened PR URL in browser"));
}
#[test]
fn pr_on_base_branch_fails_with_clear_message() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["--yes", "pr"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "cannot open PR from 'main' into itself",
        ))
        .stderr(predicate::str::contains("is not stacked").not());
}
#[test]
fn pr_on_base_branch_porcelain_reports_blocked_state() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["pr", "--porcelain"])
        .output()
        .expect("run stack pr porcelain on base branch");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["head"], "main");
    assert_eq!(json["base"], "main");
    assert_eq!(json["can_open_link"], false);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("cannot open PR from 'main' into itself")
    );
}
