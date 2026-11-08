# ADR-0068: Vault Key Rotation at Boot

## Status

Accepted.

## Context

Temper's ADR-0075 lets a server rotate the secrets vault's master key: boot with the new key in `TEMPER_VAULT_KEY` and the old one in `TEMPER_VAULT_KEY_PREVIOUS`, and every stored row only the old key opens is sealed again under the new key, on the condition that its version has not changed. `temper serve` wires this up. TemperPaw does not run `temper serve`: `temperpaw-server` builds its own vault and restored only its own tenant's rows (and the legacy `default` bucket) into the shared platform cache, opening them with the current key alone. So in TemperPaw `TEMPER_VAULT_KEY_PREVIOUS` was ignored, and a boot with a new key would have skipped every stored row as undecryptable. Other tenants' rows were not loaded at boot at all.

## Decision

- `temperpaw-server` reads `TEMPER_VAULT_KEY_PREVIOUS` and gives it to the vault with `SecretsVault::with_previous_key`. Its rules are `temper serve`'s: it requires a valid `TEMPER_VAULT_KEY` (a generated or file key would strand re-sealed rows at the next boot), and a malformed previous key stops the boot.
- Before the platform cache is filled, every tenant holding stored secrets (`ServerState::secret_tenants`) is loaded with temper's own `ServerState::load_tenant_secrets`, which opens each row with either key, re-seals old-key rows, and caches each tenant's values. TemperPaw keeps no second copy of that logic.
- This deployment's tenant (`PAW_TENANT`) and the legacy `default` bucket keep no tenant cache from that load. Their rows are served as shared platform copies, as before, and values the boot sets afterwards (deployment variables, computed endpoints) replace those copies. A tenant copy would win over them in `get_secret` and `get_tenant_secrets` and serve the stored value instead.
- The boot logs one line with `temper serve`'s wording and counts: `Secrets from database (every tenant): N kept, N re-sealed, N deferred, N unreadable, N tenants unchecked.` It is a warning when any row is unreadable or any tenant unchecked.
- The platform-cache restore opens rows with either key too, so a row whose re-seal was deferred is still served.

## Consequences

- A rotation is: set both variables, boot, read the line (all rows `re-sealed` or `kept`, none `deferred` or `unreadable`), boot again (all `kept`), then remove `TEMPER_VAULT_KEY_PREVIOUS`.
- Other tenants' stored secrets (written through the secrets API, which caches them as tenant values) are now back in the tenant cache after a restart, as under `temper serve`. Before, a restart dropped them from memory until written again.
- Without `TEMPER_VAULT_KEY_PREVIOUS` nothing changes for rows the current key opens; the new line reports them as `kept`.
