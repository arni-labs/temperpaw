# ARN-490: Package the artifact Cargo built

Rita requests a bounded fix for WASM build scripts that let Cargo use a configured target directory but verify and copy stale files from the default target directory. Required builds must fail if their output is absent, including paw-agent.

Update the existing shared helper and affected builders. Preserve current module targets and packaging destinations. Add a regression that plants a distinct stale default-path artifact, builds into another directory, and compares the packaged output with the new artifact. Cover default output and missing required output. Execute a real packaged module with the deployed-compatible kernel.

Complete one TemperPaw PR, relevant proof, the existing full review/fix/confirmation cycle, merge and applicable installation. Verify the actual revision and build command used by Codex/Astra agents through Foundry on Tensorlake, make the corrected version available without changing active dirty worktrees, and demonstrate it there. Record refresh requirements and new-copy inheritance.

No new build system, kernel changes, libSQL work, Copy cleanup, target-policy migration, or dependency-lockfile project.

Tracking: https://linear.app/arni-build/issue/ARN-490
Author: GPT-6 Astra through Codex.
