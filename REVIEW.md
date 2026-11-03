# Reviewing temperpaw

Use the installed Stack review contract, or the bundled [Stack review contract](.stack/REVIEW.md) when Stack is not installed. Review the accepted outcome and changed behavior; independently exercise the feature when useful. Report concrete defects introduced or worsened by this change, with reproduction evidence and location. No mandatory panel, review markers, JSON record or unrelated cleanup.

Apply these repository checks only where the change touches them:

- Check entity specs, Cedar policies and WASM integrations together. New sequencing should remain visible in declared transitions; avoid hidden application orchestration in the daemon.
- Exercise the changed app flow through OData and read the resulting entities back. Include the failure/retry path when affected.
- For authorization changes, test allowed and denied principals, tenant isolation and secret handling.
- For dashboard changes, use the rendered interface. For app delivery, verify the installed Genesis pinned ref when deployment is part of the task.
- Use relevant app/crate tests and `.agents/skills/verify-temperpaw/`.

Report what you tested, the revision, findings and material limits in plain language.
