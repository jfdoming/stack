const BUILD_WORKFLOW: &str = include_str!("../.github/workflows/build.yaml");
const CARGO_LOCK: &str = include_str!("../Cargo.lock");
const RELEASE_WORKFLOW: &str = include_str!("../.github/workflows/draft-release.yaml");

fn release_job_guard() -> String {
    let job = RELEASE_WORKFLOW
        .split_once("  create-draft-release:")
        .expect("release job")
        .1;
    let guard = job
        .split_once("    if: >-")
        .expect("release job guard")
        .1
        .split_once("    runs-on:")
        .expect("end of release job guard")
        .0;
    guard.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn release_step(name: &str) -> &'static str {
    let marker = format!("      - name: {name}");
    let step = RELEASE_WORKFLOW
        .split_once(&marker)
        .unwrap_or_else(|| panic!("release step '{name}'"))
        .1;
    step.split_once("\n      - name:")
        .map_or(step, |(body, _)| body)
}

#[test]
fn privileged_release_requires_a_trusted_successful_main_push() {
    let guard = release_job_guard();
    for required in [
        "github.event.workflow_run.conclusion == 'success'",
        "github.event.workflow_run.event == 'push'",
        "github.event.workflow_run.head_branch == 'main'",
        "github.event.workflow_run.head_repository.id == github.event.repository.id",
    ] {
        assert!(
            guard.contains(required),
            "missing release guard: {required}"
        );
    }
    assert!(!guard.contains("||"), "release guard must fail closed");
}

#[test]
fn release_artifact_lookup_is_scoped_to_the_triggering_run() {
    assert!(
        RELEASE_WORKFLOW.contains("RUN_ID: ${{ github.event.workflow_run.id }}"),
        "artifact lookup must use the triggering run ID"
    );
    assert!(
        RELEASE_WORKFLOW.contains("repos/${REPO}/actions/runs/${RUN_ID}/artifacts"),
        "artifact lookup must use the run-scoped endpoint"
    );
}

#[test]
fn draft_release_recovers_artifacts_without_mutating_published_releases() {
    let resolve = release_step("Resolve release tag and state");
    assert!(resolve.contains("release_state=missing"));
    assert!(resolve.contains("release_state=draft"));
    assert!(resolve.contains("release_state=published"));
    assert!(resolve.contains("--json isDraft --jq '.isDraft'"));

    for name in [
        "Download binary artifacts from CI Build run",
        "Ensure Unix binaries are executable",
        "Validate complete binary set",
        "Attach or refresh draft binaries",
    ] {
        assert!(
            release_step(name).contains("if: steps.version.outputs.release_state != 'published'"),
            "{name} must run for missing/draft releases and skip published releases"
        );
    }

    let validate = release_step("Validate complete binary set");
    for binary in [
        "stack-x86_64-unknown-linux-gnu",
        "stack-x86_64-apple-darwin",
        "stack-aarch64-apple-darwin",
        "stack-x86_64-pc-windows-msvc.exe",
    ] {
        assert!(
            validate.contains(binary),
            "draft refresh must require {binary}"
        );
    }

    let create = release_step("Create draft release if missing");
    assert!(create.contains("if: steps.version.outputs.release_state == 'missing'"));
    assert!(create.contains("gh release create"));

    let upload = release_step("Attach or refresh draft binaries");
    assert!(upload.contains("gh release upload"));
    assert!(upload.contains("--clobber"));
    assert!(
        !RELEASE_WORKFLOW.contains("release_exists"),
        "a boolean existence check cannot distinguish recoverable drafts"
    );
}

#[test]
fn existing_draft_is_retargeted_before_assets_are_replaced() {
    let retarget = release_step("Retarget existing draft release");
    assert!(
        retarget.contains("if: steps.version.outputs.release_state == 'draft'"),
        "only an existing draft should be retargeted"
    );
    assert!(retarget.contains("TAG: ${{ steps.version.outputs.tag }}"));
    assert!(
        retarget.contains("TARGET_SHA: ${{ github.event.workflow_run.head_sha }}"),
        "the release target must come from the trusted triggering run"
    );
    assert!(retarget.contains("gh release edit \"$TAG\" --target \"$TARGET_SHA\""));
    let retarget_script = retarget
        .split_once("        run: |\n")
        .expect("retarget run script")
        .1;
    assert!(
        !retarget_script.contains("${{"),
        "workflow expressions must reach the shell through quoted environment variables"
    );

    let validate_position = RELEASE_WORKFLOW
        .find("      - name: Validate complete binary set")
        .expect("validate step");
    let retarget_position = RELEASE_WORKFLOW
        .find("      - name: Retarget existing draft release")
        .expect("retarget step");
    let upload_position = RELEASE_WORKFLOW
        .find("      - name: Attach or refresh draft binaries")
        .expect("upload step");
    assert!(
        validate_position < retarget_position && retarget_position < upload_position,
        "the draft must be validated, then retargeted, then refreshed"
    );
}

#[test]
fn build_workflow_token_is_read_only() {
    assert!(
        BUILD_WORKFLOW.contains("permissions:\n  contents: read"),
        "unprivileged build jobs must not inherit a write token"
    );
}

#[test]
fn build_workflow_runs_a_pinned_rustsec_audit() {
    assert!(
        BUILD_WORKFLOW.contains(
            "uses: rustsec/audit-check@69366f33c96575abad1ee0dba8212993eecbe998 # v2.0.0"
        ),
        "CI must run the official RustSec audit action at a pinned revision"
    );
    assert!(
        BUILD_WORKFLOW.contains("token: ${{ github.token }}"),
        "the RustSec action requires the workflow token"
    );
    assert!(
        BUILD_WORKFLOW.contains("run: cargo audit --deny warnings"),
        "CI must fail on unsound, unmaintained, or yanked dependency warnings"
    );
}

#[test]
fn lockfile_excludes_known_advisory_affected_versions() {
    for (name, version) in [
        ("anyhow", "1.0.101"),
        ("lru", "0.12.5"),
        ("paste", "1.0.15"),
    ] {
        let retained = CARGO_LOCK.split("[[package]]").any(|package| {
            package.contains(&format!("name = \"{name}\""))
                && package.contains(&format!("version = \"{version}\""))
        });
        assert!(
            !retained,
            "Cargo.lock still retains advisory-affected {name} {version}"
        );
    }
}
