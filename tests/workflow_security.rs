const BUILD_WORKFLOW: &str = include_str!("../.github/workflows/build.yaml");
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
fn build_workflow_token_is_read_only() {
    assert!(
        BUILD_WORKFLOW.contains("permissions:\n  contents: read"),
        "unprivileged build jobs must not inherit a write token"
    );
}
