# Decisions

### Explicit output directory

**Decision:** Pass an explicit target directory to Cargo and use it for packaging.

**Came up because:** Cargo honors CARGO_TARGET_DIR while the builders guess a module-local path.

**Options:** Parse Cargo JSON artifact messages using a new parser dependency; resolve Cargo metadata; or pass one explicit output directory.

**Chose explicit output over parsing because:** It fixes the mismatch without adding tools to Docker and agent build environments. CARGO_TARGET_DIR remains supported, including relative paths. When unset, these builders deliberately select the module-local target directory instead of inheriting a Cargo config target-dir.

**Where:** os-apps/wasm-build-env.sh.

### Isolated worktree on the source computer

**Decision:** Use /home/tl-user/work/arn490-wasm-artifacts on arni-big.

**Came up because:** The governed Copy returned uncertain HTTP400 and exact-name reconciliation returned HTTP404; the primary checkout is dirty.

**Options:** Wait on unrelated Copy recovery, modify the primary checkout, or use a separate worktree.

**Chose a separate worktree because:** Governed Exec works and it isolates source edits without taking over Copy cleanup or overwriting ongoing work. The unresolved copy remains recorded as Computer 01a08c11-1072-7c23-9183-5e8a5c254a03.

**Where:** Branch codex/arn490-wasm-artifacts.

### Verify the build contract through execution

**Decision:** Replace obsolete inline-command assertions with the executable artifact regression and document its verification surface.

**Came up because:** The shared helper removes literal Cargo/copy text that two foundation tests required; the existing feature map has no build-artifact entry.

**Options:** Preserve the obsolete commands, assert the helper's new spelling, or retain module-coverage assertions and use the real builder regression for behavior.

**Chose executable verification because:** It catches wrong packaged bytes and missing outputs across all builders without binding the contract to duplicated shell implementation. The existing foundation tests still check module coverage.

**Where:** scripts/test-wasm-build-artifacts.sh, crates/temperpaw/tests/paw_patrol_foundation.rs, .agents/skills/verify-temperpaw/features/wasm-artifacts.md.

### Deliver the corrected build tools to Foundry's source computer

**Decision:** Roll out the merged repository checkout on arni-1 and verify it in a new Foundry Codex/Astra session.

**Came up because:** Live Railway configuration and the authenticated Foundry UI identify arni-1 as the copy source. The inherited TemperPaw checkouts are old and dirty, and Foundry currently offers no nerdsane/temperpaw repository selection.

**Options:** Overwrite inherited checkouts, expand into Foundry repository-import changes or runtime app deployments, or install a separate clean checkout that new copies inherit.

**Chose a clean inherited checkout because:** It delivers these repository build scripts to the actual agent workflow while preserving active work. The change introduces no runtime module logic or app-spec change requiring a Genesis version switch. Existing sessions/checkouts must explicitly refresh; new copies inherit the installed checkout.

**Where:** /home/tl-user/work/temperpaw-build-verified on arni-1; final Foundry session evidence on PR #509.

### Preserve newly added patrol modules during merge

**Decision:** Keep main's three effort resource delivery modules and build them through the reviewed artifact helper.

**Came up because:** Main added these modules after the reviewed head, causing one build-script merge conflict.

**Options:** Drop the new modules, restore the old artifact copying, or combine the complete module list with the shared helper.

**Chose the combined result because:** It preserves current capabilities and exact-output packaging without changing module logic. The existing review remains attributed to cd08487; the conflict resolution is verified separately under Rita's completion authorization.

**Where:** os-apps/paw-patrol/wasm/build.sh; PR #509.

### Keep the regression scoped to the migrated shell builders

**Decision:** Enumerate the thirteen affected shell builders and fail if any is missing.

**Came up because:** Main introduced dsf-twin builders whose Python generation steps are not represented by the shell fixture. Its generator already supplies an explicit Cargo target directory and copies that output.

**Options:** Expand this fixture into the unrelated generator pipeline, silently filter discovered scripts, or declare the affected builder set.

**Chose the declared set because:** It preserves every original acceptance case, includes newly added modules inside patrol, and fails when an expected builder disappears. It avoids claiming coverage of a different build pipeline.

**Where:** scripts/test-wasm-build-artifacts.sh.

### Verify packaging behavior from the Rust contract suite

**Decision:** Run the existing artifact regression from the corridor packaging contract and keep the media test focused on module inclusion.

**Came up because:** Post-merge Cargo tests exposed two remaining literal-output-filename assertions after the successful real WASM build. The shared shell loop constructs those filenames dynamically.

**Options:** Restore redundant filename strings, delete packaging coverage, or execute the existing byte-comparison regression from the contract suite.

**Chose executable coverage because:** Cargo tests now fail on incorrect packaged bytes or missing outputs without depending on how the builder spells a path. The regression covers both corridor and media builders, and existing module-inclusion checks remain. The follow-up PR is necessary because PR #509 already merged; Rita authorized the narrow regression correction.

**Where:** crates/temperpaw/tests/corridor_engine_contract.rs and crates/temperpaw/tests/paw_media_image_generation.rs.
