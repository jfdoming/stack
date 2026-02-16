# Testing Guidelines

Use this file for test strategy, scope, and execution expectations.

## Default Test Workflow
- Run `cargo test` before finalizing code changes.
- Run `cargo fmt` before committing.
- Prefer adding tests in the same module as the change (`mod tests`) when practical.

## What To Test
- Behavioural changes in CLI flows (arguments, prompts, output, error paths).
- Stack graph integrity and parent/child relationship invariants.
- Sync planning/execution edge cases (fallback paths, conflict handling).
- Database mutations that affect branch relationships.

## Integration Test Expectations
- Add/update integration tests for user-visible command behaviour changes.
- Cover non-interactive mode explicitly when command defaults/prompts change.
- Validate porcelain output shape when output contracts change.

## Quality Bar
- New behaviour should be covered by at least one new or updated test.
- Prefer deterministic tests (no network dependency assumptions).
- Keep tests task-scoped; avoid unrelated refactors in test-only commits.
