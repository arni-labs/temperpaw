<script lang="ts">
  import { onMount } from 'svelte';
  import {
    fetchSecretsSchema,
    fetchSetupStatus,
    getSecret,
    saveSecret,
    deleteSecret,
    listSecretKeys,
    connectDiscord,
    disconnectDiscord,
    connectSlack,
    disconnectSlack,
    fetchOpenAICodexStatus,
    startOpenAICodexDeviceLogin,
    pollOpenAICodexDeviceLogin,
    disconnectOpenAICodexAuth,
    type SecretSchemaEntry,
    type SetupStatus,
    type OpenAICodexAuthStatus,
  } from '$lib/api';
  import { changePassword } from '$lib/auth';

  interface VarRow {
    key: string;
    label: string;
    category: string;
    description: string;
    value: string;
    filled: boolean;
    editing: boolean;
    draft: string;
    saving: boolean;
    isCustom: boolean;
    sensitive: boolean;
  }

  let loading = $state(true);
  let status = $state<SetupStatus | null>(null);
  let vars = $state<VarRow[]>([]);
  let feedback = $state<{ type: 'error' | 'success'; message: string } | null>(null);

  // Add variable form
  let addingVar = $state(false);
  let newKey = $state('');
  let newValue = $state('');

  // Service connect states
  let connectingDiscord = $state(false);
  let connectingSlack = $state(false);
  let connectingCodex = $state(false);
  let codexStatus = $state<OpenAICodexAuthStatus | null>(null);

  // Account
  let apiKey = $state('');
  let showApiKey = $state(false);
  let currentPassword = $state('');
  let newPassword = $state('');
  let accountFeedback = $state<{ type: 'error' | 'success'; message: string } | null>(null);

  // Keys that should be masked (actual secrets). Non-secret config values show plain.
  const SENSITIVE_KEYS = new Set([
    'anthropic_api_key', 'openai_api_key', 'openai_codex_access_token', 'openai_codex_refresh_token', 'openai_codex_token', 'openrouter_api_key',
    'huggingface_api_key', 'hf_token', 'fireworks_api_key', 'sakana_fugu_api_key', 'openai_compatible_api_key', 'openai_compatible_headers_json',
    'discord_bot_token', 'slack_app_token', 'slack_bot_token', 'slack_signing_secret',
    'github_token', 'exa_api_key', 'tensorlake_api_key', 'modal_token_id', 'modal_token_secret', 'dd_api_key', 'dd_app_key', 'temper_api_key',
  ]);

  function showFeedback(type: 'error' | 'success', message: string) {
    feedback = { type, message };
    if (type === 'success') {
      setTimeout(() => { feedback = null; }, 3000);
    }
  }

  async function copyDiscordInteractionUrl() {
    const interactionUrl = status?.discord_interaction_url;
    if (!interactionUrl) return;
    try {
      await navigator.clipboard.writeText(interactionUrl);
      showFeedback('success', 'Copied Discord Interaction URL. Paste it into Discord Developer Portal -> General Information -> Interactions Endpoint URL.');
    } catch {
      showFeedback('error', 'Failed to copy the Discord Interaction URL');
    }
  }

  onMount(async () => {
    await load();
  });

  async function load() {
    loading = true;
    try {
      const [schema, existingKeys, nextStatus, savedApiKey] = await Promise.all([
        fetchSecretsSchema(),
        listSecretKeys(),
        fetchSetupStatus(),
        getSecret('temper_api_key'),
      ]);
      status = nextStatus;
      codexStatus = await fetchOpenAICodexStatus().catch(() => null);
      apiKey = savedApiKey ?? '';

      const schemaKeys = new Set(schema.map(s => s.key));
      const rows: VarRow[] = [];

      // Fetch values for all schema keys in parallel
      const valueResults = await Promise.all(
        schema.map(s => getSecret(s.key).then(v => ({ key: s.key, value: v })))
      );
      const valueMap = new Map(valueResults.map(r => [r.key, r.value]));

      for (const s of schema) {
        const val = valueMap.get(s.key) ?? '';
        rows.push({
          key: s.key,
          label: s.label,
          category: s.category,
          description: s.description,
          value: val,
          filled: !!val,
          editing: false,
          draft: '',
          saving: false,
          isCustom: false,
          sensitive: SENSITIVE_KEYS.has(s.key),
        });
      }

      // Add any custom keys not in schema
      for (const k of existingKeys) {
        if (!schemaKeys.has(k)) {
          const val = await getSecret(k);
          rows.push({
            key: k,
            label: k,
            category: 'custom',
            description: '',
            value: val ?? '',
            filled: !!val,
            editing: false,
            draft: '',
            saving: false,
            isCustom: true,
            sensitive: true,
          });
        }
      }

      vars = rows;
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'Failed to load');
    } finally {
      loading = false;
    }
  }

  function startEdit(row: VarRow) {
    row.editing = true;
    row.draft = row.value;
    vars = [...vars];
  }

  function cancelEdit(row: VarRow) {
    row.editing = false;
    row.draft = '';
    vars = [...vars];
  }

  async function saveVar(row: VarRow) {
    if (!row.draft.trim()) return;
    row.saving = true;
    vars = [...vars];
    try {
      await saveSecret(row.key, row.draft.trim());
      row.value = row.draft.trim();
      row.filled = true;
      row.editing = false;
      row.draft = '';
      status = await fetchSetupStatus();
      if (row.key.startsWith('discord_') && status?.discord_connected) {
        showFeedback('success', `${row.key} saved and Discord reconnected`);
      } else {
        showFeedback('success', `${row.key} saved`);
      }
    } catch (err) {
      showFeedback('error', `Failed to save ${row.key}: ${err instanceof Error ? err.message : 'unknown'}`);
    } finally {
      row.saving = false;
      vars = [...vars];
    }
  }

  async function removeVar(row: VarRow) {
    row.saving = true;
    vars = [...vars];
    try {
      await deleteSecret(row.key);
      if (row.isCustom) {
        vars = vars.filter(v => v.key !== row.key);
      } else {
        row.value = '';
        row.filled = false;
        row.editing = false;
        row.saving = false;
        vars = [...vars];
      }
      status = await fetchSetupStatus();
    } catch (err) {
      row.saving = false;
      vars = [...vars];
      showFeedback('error', `Failed to delete ${row.key}`);
    }
  }

  async function addVar() {
    const k = newKey.trim();
    const v = newValue.trim();
    if (!k || !v) return;
    try {
      await saveSecret(k, v);
      vars = [...vars, {
        key: k, label: k, category: 'custom', description: '',
        value: v, filled: true, editing: false, draft: '', saving: false,
        isCustom: true, sensitive: true,
      }];
      newKey = '';
      newValue = '';
      addingVar = false;
      showFeedback('success', `${k} added`);
    } catch (err) {
      showFeedback('error', `Failed to add ${k}: ${err instanceof Error ? err.message : 'unknown'}`);
    }
  }

  // Group vars by category, LLM always first
  let groupedVars = $derived.by(() => {
    const catOrder = ['llm', 'web_search', 'sandbox', 'messaging', 'integrations', 'observability', 'custom'];
    const catMap = new Map<string, VarRow[]>();
    for (const v of vars) {
      const cat = v.category || 'custom';
      if (!catMap.has(cat)) catMap.set(cat, []);
      catMap.get(cat)!.push(v);
    }
    const groups: Array<{ category: string; rows: VarRow[] }> = [];
    for (const cat of catOrder) {
      if (catMap.has(cat)) groups.push({ category: cat, rows: catMap.get(cat)! });
    }
    for (const [cat, rows] of catMap) {
      if (!catOrder.includes(cat)) groups.push({ category: cat, rows });
    }
    return groups;
  });

  // Discord connect
  async function handleConnectDiscord() {
    connectingDiscord = true;
    feedback = null;
    try {
      const botToken = vars.find(v => v.key === 'discord_bot_token')?.value ?? '';
      const publicKey = vars.find(v => v.key === 'discord_public_key')?.value ?? '';
      const guildId = vars.find(v => v.key === 'discord_guild_id')?.value ?? '';
      const feedChannel = vars.find(v => v.key === 'discord_feed_channel_id')?.value ?? '';
      const forumChannel = vars.find(v => v.key === 'discord_forum_channel_id')?.value ?? '';
      if (!botToken) { showFeedback('error', 'Set discord_bot_token first'); return; }
      const result = await connectDiscord({
        bot_token: botToken,
        public_key: publicKey || undefined,
        guild_id: guildId || undefined,
        feed_channel_id: feedChannel || undefined,
        forum_channel_id: forumChannel || undefined,
      });
      status = await fetchSetupStatus();
      const interactionUrl = result.discord_interaction_url ?? status?.discord_interaction_url;
      if (interactionUrl) {
        showFeedback('success', 'Discord connected. Copy the Interaction URL below into Discord Developer Portal -> General Information -> Interactions Endpoint URL, then save.');
      } else {
        showFeedback('success', 'Discord connected');
      }
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'Discord connection failed');
    } finally { connectingDiscord = false; }
  }

  async function handleDisconnectDiscord() {
    connectingDiscord = true;
    try {
      await disconnectDiscord();
      status = await fetchSetupStatus();
      showFeedback('success', 'Discord disconnected');
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'Failed');
    } finally { connectingDiscord = false; }
  }

  async function handleConnectSlack() {
    connectingSlack = true;
    feedback = null;
    try {
      const appToken = vars.find(v => v.key === 'slack_app_token')?.value ?? '';
      const botToken = vars.find(v => v.key === 'slack_bot_token')?.value ?? '';
      const signing = vars.find(v => v.key === 'slack_signing_secret')?.value ?? '';
      if (!appToken || !botToken) { showFeedback('error', 'Set slack_app_token and slack_bot_token first'); return; }
      await connectSlack({
        app_token: appToken, bot_token: botToken,
        signing_secret: signing || undefined,
      });
      status = await fetchSetupStatus();
      showFeedback('success', 'Slack connected');
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'Slack connection failed');
    } finally { connectingSlack = false; }
  }

  async function handleDisconnectSlack() {
    connectingSlack = true;
    try {
      await disconnectSlack();
      status = await fetchSetupStatus();
      showFeedback('success', 'Slack disconnected');
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'Failed');
    } finally { connectingSlack = false; }
  }

  async function handleStartCodexLogin() {
    connectingCodex = true;
    feedback = null;
    try {
      codexStatus = await startOpenAICodexDeviceLogin();
      showFeedback('success', 'OpenAI Codex device login started');
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'OpenAI Codex login failed');
    } finally { connectingCodex = false; }
  }

  async function handlePollCodexLogin() {
    connectingCodex = true;
    feedback = null;
    try {
      codexStatus = await pollOpenAICodexDeviceLogin();
      status = await fetchSetupStatus();
      if (codexStatus.configured) {
        showFeedback('success', 'OpenAI Codex connected');
        await load();
      }
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'OpenAI Codex polling failed');
    } finally { connectingCodex = false; }
  }

  async function handleDisconnectCodex() {
    connectingCodex = true;
    try {
      codexStatus = await disconnectOpenAICodexAuth();
      status = await fetchSetupStatus();
      await load();
      showFeedback('success', 'OpenAI Codex disconnected');
    } catch (err) {
      showFeedback('error', err instanceof Error ? err.message : 'OpenAI Codex disconnect failed');
    } finally { connectingCodex = false; }
  }

  async function updatePassword() {
    accountFeedback = null;
    try {
      await changePassword(currentPassword, newPassword);
      currentPassword = '';
      newPassword = '';
      accountFeedback = { type: 'success', message: 'Password updated' };
      setTimeout(() => { accountFeedback = null; }, 3000);
    } catch (err) {
      accountFeedback = { type: 'error', message: err instanceof Error ? err.message : 'Failed' };
    }
  }

  function mask(val: string): string {
    if (!val) return '';
    if (val.length <= 8) return '\u2022'.repeat(val.length);
    return val.slice(0, 4) + '\u2022'.repeat(Math.min(val.length - 4, 12));
  }

  const CAT_LABELS: Record<string, string> = {
    llm: 'LLM',
    web_search: 'Web Search',
    sandbox: 'Sandbox',
    messaging: 'Messaging',
    integrations: 'Integrations',
    observability: 'Observability',
    custom: 'Custom',
  };
</script>

<svelte:head>
  <title>Settings &middot; Temper Paw</title>
</svelte:head>

<div class="page">
  <h1 class="page-title">SETTINGS</h1>

  {#if loading}
    <p class="dim">Loading...</p>
  {:else}
    {#if feedback}
      <div class="toast" class:toast-err={feedback.type === 'error'}>{feedback.message}</div>
    {/if}

    <!-- Variable list grouped by category -->
    <div class="var-list">
      {#each groupedVars as group}
        <div class="cat-header">
          <span class="cat-label">{CAT_LABELS[group.category] ?? group.category}</span>
          <!-- Inline connect/disconnect for messaging -->
          {#if group.category === 'messaging'}
            <div class="cat-actions">
              {#if status?.discord_connected}
                <button class="cat-act" onclick={handleDisconnectDiscord} disabled={connectingDiscord}>
                  <span class="cat-dot cat-dot--on"></span> Discord <span class="cat-act-label">Disconnect</span>
                </button>
              {:else}
                <button class="cat-act" onclick={handleConnectDiscord} disabled={connectingDiscord}>
                  <span class="cat-dot"></span> Discord <span class="cat-act-label">Connect</span>
                </button>
              {/if}
              {#if status?.slack_connected}
                <button class="cat-act" onclick={handleDisconnectSlack} disabled={connectingSlack}>
                  <span class="cat-dot cat-dot--on"></span> Slack <span class="cat-act-label">Disconnect</span>
                </button>
              {:else}
                <button class="cat-act" onclick={handleConnectSlack} disabled={connectingSlack}>
                  <span class="cat-dot"></span> Slack <span class="cat-act-label">Connect</span>
                </button>
              {/if}
            </div>
          {/if}
          {#if group.category === 'llm'}
            <div class="cat-actions">
              {#if codexStatus?.configured}
                <button class="cat-act" onclick={handleDisconnectCodex} disabled={connectingCodex}>
                  <span class="cat-dot cat-dot--on"></span> Codex <span class="cat-act-label">Disconnect</span>
                </button>
              {:else}
                <button class="cat-act" onclick={handleStartCodexLogin} disabled={connectingCodex}>
                  <span class="cat-dot"></span> Codex <span class="cat-act-label">Sign in</span>
                </button>
              {/if}
            </div>
          {/if}
        </div>
        {#if group.category === 'llm' && codexStatus?.user_code}
          <div class="cat-hint">
            <div class="interaction-copy-row">
              <span class="interaction-copy-label">OpenAI Codex Device Code</span>
              <button class="act" onclick={handlePollCodexLogin} disabled={connectingCodex}>Check</button>
            </div>
            <code class="interaction-url">{codexStatus.user_code}</code>
            <div><a href={codexStatus.verification_url} target="_blank" rel="noreferrer">{codexStatus.verification_url}</a></div>
          </div>
        {/if}
        {#if group.category === 'messaging'}
          <div class="cat-hint">Saving Discord credentials applies them immediately. Use Connect only to retry manually.</div>
        {/if}
        {#if group.category === 'messaging' && status?.discord_interaction_url}
          <div class="cat-hint">
            <div class="interaction-copy-row">
              <span class="interaction-copy-label">Discord Interaction URL</span>
              <button class="act" onclick={copyDiscordInteractionUrl}>Copy</button>
            </div>
            <code class="interaction-url">{status.discord_interaction_url}</code>
            <div>Paste this into Discord Developer Portal -> General Information -> Interactions Endpoint URL, then click Save Changes.</div>
          </div>
        {/if}
        {#each group.rows as row (row.key)}
          <div class="var-row">
            <div class="var-main">
              <span class="var-dot" class:var-dot--on={row.filled}></span>
              <span class="var-key">{row.key}</span>
              <span class="var-val">
                {#if row.filled}
                  {row.sensitive ? mask(row.value) : row.value}
                {:else}
                  <span class="var-unset">--</span>
                {/if}
              </span>
              <div class="var-actions">
                {#if row.editing}
                  <button class="act" onclick={() => cancelEdit(row)} disabled={row.saving}>Cancel</button>
                {:else}
                  <button class="act" onclick={() => startEdit(row)}>{row.filled ? 'Edit' : 'Set'}</button>
                  {#if row.filled}
                    <button class="act act-danger" onclick={() => removeVar(row)} disabled={row.saving}>Del</button>
                  {/if}
                {/if}
              </div>
            </div>
            {#if row.description && !row.editing}
              <div class="var-desc">{row.description}</div>
            {/if}
            {#if row.editing}
              <form class="var-edit" onsubmit={(e) => { e.preventDefault(); saveVar(row); }}>
                <input
                  class="var-input"
                  type={row.sensitive ? 'password' : 'text'}
                  bind:value={row.draft}
                  placeholder={row.sensitive ? 'Enter value' : row.value || 'Enter value'}
                  disabled={row.saving}
                />
                <button class="btn-sm" type="submit" disabled={!row.draft.trim() || row.saving}>
                  {row.saving ? '...' : 'Save'}
                </button>
              </form>
            {/if}
          </div>
        {/each}
      {/each}

      <!-- Add variable -->
      {#if addingVar}
        <form class="add-form" onsubmit={(e) => { e.preventDefault(); addVar(); }}>
          <input class="var-input var-input-key" bind:value={newKey} placeholder="KEY_NAME" />
          <input class="var-input" type="password" bind:value={newValue} placeholder="Value" />
          <button class="btn-sm" type="submit" disabled={!newKey.trim() || !newValue.trim()}>Add</button>
          <button class="act" type="button" onclick={() => { addingVar = false; newKey = ''; newValue = ''; }}>Cancel</button>
        </form>
      {:else}
        <button class="add-btn" onclick={() => addingVar = true}>+ Add variable</button>
      {/if}
    </div>

    <!-- Account -->
    <div class="section">
      <span class="cat-label">Account</span>
      {#if accountFeedback}
        <span class="inline-fb" class:fb-err={accountFeedback.type === 'error'} class:fb-ok={accountFeedback.type === 'success'}>{accountFeedback.message}</span>
      {/if}
      <div class="acct-row">
        <span class="var-key">temper_api_key</span>
        <span class="var-val">{showApiKey ? apiKey : mask(apiKey)}</span>
        <button class="act" onclick={() => showApiKey = !showApiKey}>{showApiKey ? 'Hide' : 'Show'}</button>
      </div>
      <form class="pw-form" onsubmit={(e) => { e.preventDefault(); updatePassword(); }}>
        <input class="var-input" type="password" bind:value={currentPassword} placeholder="Current password" />
        <input class="var-input" type="password" bind:value={newPassword} placeholder="New password" />
        <button class="btn-sm" type="submit" disabled={!currentPassword || !newPassword}>Change password</button>
      </form>
    </div>
  {/if}
</div>

<style>
  .page { max-width: 720px; }

  .page-title {
    font-family: var(--font-sans);
    font-size: var(--text-xl);
    color: var(--text-1);
    margin-bottom: var(--sp-6);
    letter-spacing: 0.1em;
  }

  .dim {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
  }

  .toast {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--accent);
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid rgba(52,211,153,0.15);
    border-radius: var(--radius);
    margin-bottom: var(--sp-4);
  }

  .toast-err {
    color: var(--status-error);
    border-color: rgba(248,113,113,0.15);
  }

  /* ── Var list ── */
  .var-list {
    display: flex;
    flex-direction: column;
    margin-bottom: var(--sp-6);
    border-bottom: 1px solid var(--border);
    padding-bottom: var(--sp-2);
  }

  /* ── Category header ── */
  .cat-header {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-4) 0 var(--sp-1) 0;
  }

  .cat-header:first-child {
    padding-top: 0;
  }

  .cat-label {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .cat-actions {
    display: flex;
    gap: var(--sp-3);
    margin-left: auto;
  }

  .cat-act {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-2);
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px var(--sp-1);
    border-radius: var(--radius);
  }

  .cat-act:hover { color: var(--text-1); }
  .cat-act:disabled { opacity: 0.3; cursor: not-allowed; }

  .cat-act-label {
    color: var(--text-3);
    margin-left: 2px;
  }

  .cat-dot {
    width: 5px; height: 5px;
    border-radius: 50%;
    background: var(--text-3);
    opacity: 0.4;
  }

  .cat-dot--on {
    background: var(--accent);
    opacity: 1;
  }

  .cat-hint {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
    padding: 0 0 var(--sp-1) 0;
    word-break: break-all;
  }

  /* ── Variable row ── */
  .var-row {
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
    padding: var(--sp-1) 0;
  }

  .var-main {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-height: 26px;
  }

  .var-dot {
    width: 6px; height: 6px;
    border-radius: 50%;
    background: var(--text-3);
    flex-shrink: 0;
    opacity: 0.3;
  }

  .var-dot--on {
    background: var(--accent);
    opacity: 1;
  }

  .var-key {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-1);
    white-space: nowrap;
    min-width: 0;
  }

  .var-val {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-2);
    margin-left: auto;
    text-align: right;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 200px;
  }

  .interaction-copy-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    justify-content: space-between;
    margin-bottom: var(--sp-1);
  }

  .interaction-copy-label {
    color: var(--text-2);
  }

  .interaction-url {
    display: block;
    color: var(--text-1);
    margin-bottom: var(--sp-1);
    word-break: break-all;
  }

  .var-unset {
    color: var(--text-3);
    opacity: 0.4;
  }

  .var-desc {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
    padding-left: calc(6px + var(--sp-2));
    opacity: 0.7;
  }

  .var-actions {
    display: flex;
    gap: var(--sp-1);
    flex-shrink: 0;
    margin-left: var(--sp-2);
  }

  .act {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px var(--sp-1);
    border-radius: var(--radius);
  }

  .act:hover { color: var(--text-1); }
  .act:disabled { opacity: 0.3; cursor: not-allowed; }
  .act-danger:hover { color: var(--status-error); }

  /* ── Inline edit ── */
  .var-edit {
    display: flex;
    gap: var(--sp-2);
    padding: var(--sp-1) 0 0 calc(6px + var(--sp-2));
  }

  .var-input {
    flex: 1;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--sp-1) var(--sp-2);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-1);
  }

  .var-input::placeholder { color: var(--text-3); }
  .var-input:focus { outline: none; border-color: var(--text-2); }

  .var-input-key { max-width: 200px; text-transform: uppercase; }

  .btn-sm {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    letter-spacing: 0.06em;
    padding: var(--sp-1) var(--sp-3);
    border-radius: var(--radius);
    border: 1px solid var(--text-1);
    background: var(--text-1);
    color: var(--bg);
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .btn-sm:disabled { opacity: 0.25; cursor: not-allowed; }

  /* ── Add variable ── */
  .add-form {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) 0;
    border-top: 1px solid var(--border);
  }

  .add-btn {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
    background: none;
    border: none;
    cursor: pointer;
    padding: var(--sp-2) 0;
    text-align: left;
  }

  .add-btn:hover { color: var(--text-1); }

  /* ── Section ── */
  .section {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding-bottom: var(--sp-4);
  }

  .inline-fb {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }

  .fb-err { color: var(--status-error); }
  .fb-ok { color: var(--accent); }

  .acct-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) 0;
  }

  .acct-row .var-val { flex: 1; text-align: left; }

  .pw-form {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
    padding: var(--sp-1) 0;
  }

  @media (max-width: 640px) {
    .page { max-width: 100%; }
    .var-main { flex-wrap: wrap; }
    .var-val { max-width: none; text-align: left; margin-left: 0; width: 100%; padding-left: calc(6px + var(--sp-2)); }
    .var-actions { width: 100%; padding-left: calc(6px + var(--sp-2)); }
    .cat-actions { flex-wrap: wrap; gap: var(--sp-2); }
    .cat-header { flex-wrap: wrap; }
    .pw-form { flex-direction: column; align-items: stretch; }
    .var-edit { padding-left: 0; flex-direction: column; }
    .add-form { flex-wrap: wrap; }
    .var-input-key { max-width: none; }
    .acct-row { flex-wrap: wrap; }
    .acct-row .var-val { flex: none; width: 100%; }
  }
</style>
