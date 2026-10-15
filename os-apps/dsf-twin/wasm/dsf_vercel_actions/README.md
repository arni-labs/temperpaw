# Vercel project actions

These four `ResourceAction` implementations operate on a registered
`DsfVercelProject`. The shared runtime loads and hashes the immutable configuration
File and checks the linked Effort and exact change proof before execution. This
crate issues provider requests and returns results; it never dispatches actions.

The target contains `project_id`, `account_id` (the Vercel team ID), `project_name`,
`git_repository_id`, `token_secret`, and an optional `allowed_aliases` list. Every
phase reads the project under that team and checks both returned identities.

| Action | Change | Provider write |
| --- | --- | --- |
| Deploy | `target`, `baseline_deployment_id`, `not_before_ms`; exact revision comes from the resource action | POST `/v13/deployments` with the GitHub commit and operation key/sequence metadata |
| ApplyConfiguration | `target` plus one or more supported settings below | PATCH `/v9/projects/{project_id}` |
| Rollback | `target: production`, `deployment_id`, `baseline_deployment_id`; exact revision comes from the resource action | POST `/v1/projects/{project_id}/rollback/{deployment_id}` |
| SetAlias | `target`, `alias`, `deployment_id`, `revision` | POST `/v2/deployments/{deployment_id}/aliases` |

`Deploy` omits the provider target for previews, as required by the Vercel API.
It adopts only a deployment with the exact project, commit, operation key,
sequence, and creation window. A lost create response never causes another create
request. The bounded search refuses to infer absence from an incomplete page.

Supported project settings are `build_command`, `install_command`,
`output_directory`, `root_directory`, `framework`, and `node_version`. Values are
strings; omitted or null fields remain unchanged. These settings apply to future
builds throughout the project. The `target` field binds the accepted action; it
does not make project settings specific to that deployment environment. Readback
checks the requested fields. Health verification checks the current production
revision for continuity and does not claim that changing settings rebuilt it.

Rollback accepts the provider's empty HTTP 201 response, then reads the actual
production pointer. Alias assignment checks the selected deployment, registered
alias allowlist, and current alias ownership. Both reconcile an uncertain write
without repeating it against a potentially changed production or alias owner.

Verification reads the exact provider state, probes the affected deployment or
alias, and requires a Datadog span for that probe and revision. Preview and custom
alias probes support only a page on that origin or its health endpoint. They do
not use backend credentials. Provider responses with missing identities,
malformed facts, or mismatched commits never count as success.

Tests use the real shared runtime with a recording HTTP host. They cover request
shapes, lost replies, stale identities, project and alias ownership, and matching
health/Datadog evidence. The parent integration suite must also run the generated
stage modules in the actual WASM engine and verify an authorized live path before
release; these HTTP fixtures alone are not that proof.

API contracts were checked against the official
[Vercel SDK](https://github.com/vercel/sdk/tree/main/src/funcs), specifically
`deploymentsCreateDeployment`, `projectsUpdateProject`, `projectsRequestRollback`,
`aliasesAssignAlias`, `aliasesGetAlias`, and `projectsGetProject`.
