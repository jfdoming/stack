use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BranchView {
    pub name: String,
    pub parent: Option<String>,
    pub last_synced_head_sha: Option<String>,
    pub cached_pr_number: Option<i64>,
    pub cached_pr_state: Option<String>,
    pub exists_in_git: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationView {
    pub kind: String,
    pub branch: String,
    pub onto: Option<String>,
    pub details: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncPlanView {
    pub base_branch: String,
    pub operations: Vec<OperationView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorIssueView {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub branch: Option<String>,
}

pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
