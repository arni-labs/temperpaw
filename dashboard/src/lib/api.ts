import { pawPatrolView, type AppViewManifest } from '$lib/app-views/paw-patrol';
import { entitySetToEntityType } from '$lib/dashboard-format';

const BASE = ''; // relative — proxied by Vite in dev, served by tower-http in prod

// Default headers for all OData requests.
const HEADERS: Record<string, string> = {
  'x-temper-principal-kind': 'human',
  'x-temper-principal-id': 'dashboard'
};

export async function apiFetch(input: string, init: RequestInit = {}): Promise<Response> {
  const headers = {
    ...HEADERS,
    ...(init.headers as Record<string, string> | undefined)
  };

  return fetch(input, {
    ...init,
    headers,
    credentials: 'same-origin'
  });
}

/**
 * Flatten a Temper OData entity response.
 * Temper returns { entity_type, entity_id, status, fields: { Id, Status, ... }, counters, booleans, ... }.
 * We merge `fields`, `counters`, and `booleans` into a single flat object for simpler consumption.
 */
function flattenEntity(raw: Record<string, unknown>): Record<string, unknown> {
  const fields = (raw.fields ?? {}) as Record<string, unknown>;
  const counters = (raw.counters ?? {}) as Record<string, unknown>;
  const booleans = (raw.booleans ?? {}) as Record<string, unknown>;
  return {
    _entity_type: raw.entity_type,
    _entity_id: raw.entity_id,
    _events: raw.events ?? [],
    _sequence_nr: raw.sequence_nr,
    _total_event_count: raw.total_event_count,
    ...fields,
    ...counters,
    ...booleans,
  };
}

function escapeODataString(value: string): string {
  return value.replaceAll("'", "''");
}

function rowEntityId(row: Record<string, unknown>): string {
  const fields = (row.fields ?? {}) as Record<string, unknown>;
  return String(row.Id ?? row.id ?? row.entity_id ?? row._entity_id ?? fields.Id ?? fields.id ?? '');
}

function entityResource(entitySet: string, id: string): string {
  return `${entitySet}('${escapeODataString(id)}')`;
}

export async function queryEntities(
  entitySet: string,
  filter?: string,
  orderby?: string,
  top?: number
): Promise<Record<string, unknown>[]> {
  let url = `${BASE}/tdata/${entitySet}`;
  const params = new URLSearchParams();
  if (filter) params.set('$filter', filter);
  if (orderby) params.set('$orderby', orderby);
  if (top) params.set('$top', top.toString());
  const qs = params.toString();
  if (qs) url += `?${qs}`;

  const res = await apiFetch(url);
  if (!res.ok) {
    throw new Error(`OData query failed: ${res.status} ${res.statusText}`);
  }
  const data = await res.json();
  const raw = (data.value || []) as Record<string, unknown>[];
  return raw.map(flattenEntity);
}

export async function createEntity(
  entitySet: string,
  body: Record<string, unknown> = {}
): Promise<Record<string, unknown>> {
  const res = await apiFetch(`${BASE}/tdata/${entitySet}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!res.ok) {
    throw new Error(`OData create failed: ${res.status} ${res.statusText}`);
  }
  const raw = await res.json();
  return flattenEntity(raw);
}

export async function postEntityAction(
  entitySet: string,
  id: string,
  action: string,
  body: Record<string, unknown> = {},
  namespace = 'Temper'
): Promise<Record<string, unknown>> {
  let target = `${entityResource(entitySet, id)}/${namespace}.${action}`;
  const entity = await apiFetch(`${BASE}/tdata/${entityResource(entitySet, id)}`);
  if (entity.ok) {
    const raw = await entity.json().catch(() => ({}));
    const actions = Array.isArray(raw['@odata.actions']) ? raw['@odata.actions'] : [];
    const advertised = actions.find(
      (item: Record<string, unknown>) =>
        item.name === action && typeof item.target === 'string'
    );
    if (advertised?.target) {
      target = advertised.target as string;
    }
  }

  const path = target.startsWith('/') ? target : `/tdata/${target}`;
  const res = await apiFetch(`${BASE}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!res.ok) {
    throw Object.assign(new Error(`OData action failed: ${res.status} ${res.statusText}`), { status: res.status });
  }
  const raw = await res.json().catch(() => ({}));
  return typeof raw === 'object' && raw !== null ? flattenEntity(raw as Record<string, unknown>) : {};
}

export async function fetchAppViewManifest(name: string): Promise<AppViewManifest | null> {
  if (name === pawPatrolView.name) return pawPatrolView;
  return null;
}

export async function fetchDecisions(status?: string): Promise<DecisionsResponse> {
  // Temper platform serves decisions at /api/decisions (global, not tenant-scoped)
  let url = `${BASE}/api/decisions`;
  if (status) url += `?status=${status}`;
  const res = await apiFetch(url);
  if (!res.ok) return { decisions: [], total: 0, pending_count: 0, approved_count: 0, denied_count: 0 };
  return res.json();
}

export async function fetchPolicies(): Promise<PolicyEntry[]> {
  // Temper platform uses /policies/list and returns { policies: [...] }
  const res = await apiFetch(`${BASE}/api/tenants/default/policies/list`);
  if (!res.ok) return [];
  const data = await res.json();
  return Array.isArray(data) ? data : (data.policies ?? data.value ?? []);
}

export interface DecisionsResponse {
  decisions: PendingDecision[];
  total: number;
  pending_count: number;
  approved_count: number;
  denied_count: number;
}

export interface PendingDecision {
  id: string;
  tenant: string;
  agent_id: string;
  action: string;
  resource_type: string;
  resource_id: string;
  resource_attrs?: Record<string, unknown>;
  denial_reason: string;
  module_name?: string;
  agent_type?: string;
  created_at: string;
  status: string;
  decided_by?: string;
  decided_at?: string;
  generated_policy?: string;
}

export interface PolicyEntry {
  policy_id: string;
  tenant: string;
  cedar_text: string;
  enabled: boolean;
  created_at?: string;
  created_by?: string;
  source?: string;
}

export interface SessionHistoryEntry {
  timestamp: string;
  tenant: string;
  entity_type: string;
  entity_id: string;
  action: string;
  success: boolean;
  from_status: string;
  to_status: string;
  error: string | null;
  authz_denied: boolean;
  denied_resource: string | null;
  /** Cedar policy IDs that contributed to the authorization decision (allow or deny).
   *  Available after Temper ADR-0039 (authz policy traceability). */
  matched_policy_ids: string[] | null;
}

export async function fetchSessionHistory(
  entityId: string,
  entityType: string = 'Session',
  limit: number = 200
): Promise<SessionHistoryEntry[]> {
  const params = new URLSearchParams();
  if (entityType) params.set('entity_type', entityType);
  params.set('limit', limit.toString());
  const url = `${BASE}/observe/agents/system/history?${params.toString()}`;
  const res = await apiFetch(url);
  if (!res.ok) return [];
  const data = await res.json();
  const history = data.history ?? data ?? [];
  // Filter to only events for this specific entity
  return (Array.isArray(history) ? history : []).filter(
    (e: SessionHistoryEntry) => e.entity_id === entityId
  );
}

export async function fetchEntityHistory(
  entitySet: string,
  entityId: string,
  limit: number = 200
): Promise<SessionHistoryEntry[]> {
  return fetchSessionHistory(entityId, entitySetToEntityType(entitySet), limit);
}

export async function queryTeams(): Promise<Record<string, unknown>[]> {
  return queryEntities('Teams');
}

export async function queryAgentsForTeam(teamId: string): Promise<Record<string, unknown>[]> {
  return queryEntities('Agents', `team_id eq '${teamId}'`);
}

export async function getEntity(
  entitySet: string,
  id: string
): Promise<Record<string, unknown>> {
  const quotedId = encodeURIComponent(escapeODataString(id));
  const res = await apiFetch(`${BASE}/tdata/${entitySet}('${quotedId}')`);
  if (res.ok) {
    const raw = await res.json();
    return flattenEntity(raw);
  }

  const directError = `${res.status} ${res.statusText}`.trim();
  const filter = `Id eq '${escapeODataString(id)}'`;
  const filtered = await queryEntities(entitySet, filter, undefined, 1).catch(() => []);
  const filteredMatch = filtered.find((row) => rowEntityId(row) === id);
  if (filteredMatch) {
    return filteredMatch;
  }

  const listed = await queryEntities(entitySet, undefined, undefined, 100).catch(() => []);
  const listedMatch = listed.find((row) => rowEntityId(row) === id);
  if (listedMatch) {
    return listedMatch;
  }

  throw new Error(`Could not load ${entitySet} ${id} (${directError})`);
}

/** OS App entry from the Temper platform catalog. */
export interface OsAppEntry {
  name: string;
  description: string;
  entity_types: string[];
  version: string;
  app_guide: string | null;
  /** @deprecated Use app_guide */ skill_guide?: string | null;
}

/** Fetch all registered OS apps from the platform catalog. */
export async function fetchOsApps(): Promise<OsAppEntry[]> {
  const res = await apiFetch(`${BASE}/observe/os-apps`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.apps ?? [];
}

/** Fetch the APP.md guide for a specific OS app. */
export async function fetchOsAppGuide(name: string): Promise<string> {
  const res = await apiFetch(`${BASE}/observe/os-apps/${encodeURIComponent(name)}`);
  if (!res.ok) return '';
  const data = await res.json();
  return data.guide ?? '';
}

/**
 * Fetch raw file content by file ID (e.g. soul markdown, skill markdown).
 */
export async function fetchFileContent(fileId: string): Promise<string> {
  const res = await apiFetch(`${BASE}/tdata/Files('${fileId}')/$value`);
  if (!res.ok) return '';
  return res.text();
}

// ──────────────────── Setup & Transport API ────────────────────

export interface SetupStatus {
  has_anthropic_key: boolean;
  llm_provider: string | null;
  has_discord: boolean;
  has_slack: boolean;
  has_agents: boolean;
  agent_count: number;
  has_personalized_soul: boolean;
  discord_connected: boolean;
  slack_connected: boolean;
  discord_interaction_url?: string;
}

export interface SecretSchemaEntry {
  key: string;
  category: string;
  label: string;
  required: boolean;
  description: string;
}

export interface OpenAICodexAuthStatus {
  configured: boolean;
  status?: string;
  verification_url?: string;
  user_code?: string;
  expires_at_ms?: string;
  account_id?: string;
  last_error?: string;
}

export async function fetchSecretsSchema(): Promise<SecretSchemaEntry[]> {
  const res = await apiFetch(`${BASE}/paw/setup/secrets/schema`);
  if (!res.ok) return [];
  return res.json();
}

export async function fetchSetupStatus(): Promise<SetupStatus> {
  const res = await apiFetch(`${BASE}/paw/setup/status`);
  if (!res.ok) throw new Error(`Setup status failed: ${res.status}`);
  return res.json();
}

export async function fetchOpenAICodexStatus(): Promise<OpenAICodexAuthStatus> {
  const res = await apiFetch(`${BASE}/paw/setup/openai-codex/status`);
  if (!res.ok) throw new Error(`OpenAI Codex status failed: ${res.status}`);
  return res.json();
}

export async function startOpenAICodexDeviceLogin(): Promise<OpenAICodexAuthStatus> {
  const res = await apiFetch(`${BASE}/paw/setup/openai-codex/device-login`, { method: 'POST' });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `OpenAI Codex device login failed: ${res.status}`);
  }
  return res.json();
}

export async function pollOpenAICodexDeviceLogin(): Promise<OpenAICodexAuthStatus> {
  const res = await apiFetch(`${BASE}/paw/setup/openai-codex/poll`, { method: 'POST' });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `OpenAI Codex poll failed: ${res.status}`);
  }
  return res.json();
}

export async function disconnectOpenAICodexAuth(): Promise<OpenAICodexAuthStatus> {
  const res = await apiFetch(`${BASE}/paw/setup/openai-codex/disconnect`, { method: 'POST' });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `OpenAI Codex disconnect failed: ${res.status}`);
  }
  return res.json();
}

export async function saveSecret(key: string, value: string): Promise<void> {
  const res = await apiFetch(`${BASE}/paw/setup/secrets`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify({ key, value }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `Save secret failed: ${res.status}`);
  }
}

export async function listSecretKeys(): Promise<string[]> {
  const res = await apiFetch(`${BASE}/paw/setup/secrets`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.keys ?? [];
}

export async function getSecret(key: string): Promise<string | null> {
  const res = await apiFetch(`${BASE}/paw/setup/secrets/${encodeURIComponent(key)}`);
  if (!res.ok) return null;
  const data = await res.json();
  return data.value ?? null;
}

export async function deleteSecret(key: string): Promise<void> {
  await apiFetch(`${BASE}/paw/setup/secrets/${encodeURIComponent(key)}`, {
    method: 'DELETE',
  });
}

export interface SoulTemplate {
  name: string;
  description: string;
  path: string;
}

export async function getSoulTemplates(): Promise<SoulTemplate[]> {
  const res = await apiFetch(`${BASE}/paw/souls/templates`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.templates ?? [];
}

export interface UserInterview {
  name: string;
  about_you: string;
  ideal_paw: string;
  followup_answers: [string, string][];
}

export interface GeneratedSoul {
  soul_md: string;
  style_md: string;
  user_md: string;
  summary: string;
}

export interface CurrentSoul {
  summary: string;
  content: string;
}

export async function getCurrentSoul(): Promise<CurrentSoul | null> {
  const res = await apiFetch(`${BASE}/paw/setup/soul`);
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`Get soul failed: ${res.status}`);
  return res.json();
}

export async function generateSoulPreview(params: {
  interview: UserInterview;
  previous_summary?: string;
  feedback?: string;
}): Promise<GeneratedSoul> {
  const res = await apiFetch(`${BASE}/paw/setup/soul/generate`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify(params)
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `Soul generation failed: ${res.status}`);
  }
  return res.json();
}

export async function saveGeneratedSoul(generated: GeneratedSoul): Promise<void> {
  const res = await apiFetch(`${BASE}/paw/setup/soul/save`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify(generated)
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `Saving soul failed: ${res.status}`);
  }
}

export interface CreateAgentParams {
  name: string;
  role?: string;
  soul_template?: string;
  model?: string;
  tools_enabled?: string;
  max_turns?: string;
}

export async function createAgent(params: CreateAgentParams): Promise<{ agent_id: string }> {
  const res = await apiFetch(`${BASE}/paw/agents/create`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify(params),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `Create agent failed: ${res.status}`);
  }
  return res.json();
}

export interface TransportStatusResponse {
  discord: { status: string; guild_id?: string; message?: string };
  slack: { status: string; message?: string };
}

export interface DiscordConnectResponse {
  status: string;
  discord_interaction_url?: string;
}

export async function getTransportStatus(): Promise<TransportStatusResponse> {
  const res = await apiFetch(`${BASE}/paw/transports/status`);
  if (!res.ok) throw new Error(`Transport status failed: ${res.status}`);
  return res.json();
}

export async function connectDiscord(params: {
  bot_token: string;
  public_key?: string;
  guild_id?: string;
  feed_channel_id?: string;
  forum_channel_id?: string;
}): Promise<DiscordConnectResponse> {
  const res = await apiFetch(`${BASE}/paw/transports/discord/connect`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify(params),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `Connect Discord failed: ${res.status}`);
  }
  return res.json();
}

export async function disconnectDiscord(): Promise<void> {
  await apiFetch(`${BASE}/paw/transports/discord/disconnect`, {
    method: 'POST',
  });
}

export async function connectSlack(params: {
  app_token: string;
  bot_token: string;
  signing_secret?: string;
}): Promise<void> {
  const res = await apiFetch(`${BASE}/paw/transports/slack/connect`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify(params),
  });
  if (!res.ok) throw new Error(`Connect Slack failed: ${res.status}`);
}

export async function disconnectSlack(): Promise<void> {
  await apiFetch(`${BASE}/paw/transports/slack/disconnect`, {
    method: 'POST',
  });
}

// ──────────────────── Railway Integration API ────────────────────

export interface RailwayStatus {
  configured: boolean;
  can_update: boolean;
  project_id: string | null;
  environment_id: string | null;
  service_id: string | null;
  otel_service_id: string | null;
  datadog_runtime_agent_service_id: string | null;
}

export async function getRailwayStatus(): Promise<RailwayStatus> {
  const res = await apiFetch(`${BASE}/paw/infra/railway/status`);
  if (!res.ok) return { configured: false, can_update: false, project_id: null, environment_id: null, service_id: null, otel_service_id: null, datadog_runtime_agent_service_id: null };
  return res.json();
}

export async function setRailwayVar(service: string, key: string, value: string): Promise<void> {
  const res = await apiFetch(`${BASE}/paw/infra/railway/set-var`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify({ service, key, value }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `Set Railway var failed: ${res.status}`);
  }
}

// ──────────────────── Version + Updates API ────────────────────

export interface VersionInfo {
  version: string;
  sha: string;
}

export interface UpdateCheck {
  current_version: string;
  latest_version: string | null;
  latest_sha: string | null;
  update_available: boolean;
  release_url: string | null;
  release_notes: string | null;
}

export async function fetchVersion(): Promise<VersionInfo> {
  const res = await apiFetch(`${BASE}/paw/version`);
  if (!res.ok) return { version: 'unknown', sha: 'unknown' };
  return res.json();
}

export async function checkForUpdates(): Promise<UpdateCheck> {
  const res = await apiFetch(`${BASE}/paw/infra/updates`);
  if (!res.ok) {
    return {
      current_version: 'unknown',
      latest_version: null,
      latest_sha: null,
      update_available: false,
      release_url: null,
      release_notes: null,
    };
  }
  return res.json();
}

export interface EdgeBuild {
  available: boolean;
  sha: string | null;
  short_sha: string | null;
  message: string | null;
  committed_at: string | null;
}

export async function checkEdgeBuild(): Promise<EdgeBuild> {
  const res = await apiFetch(`${BASE}/paw/infra/edge`);
  if (!res.ok) {
    return { available: false, sha: null, short_sha: null, message: null, committed_at: null };
  }
  return res.json();
}

export async function triggerRedeploy(imageTag?: 'latest' | 'edge'): Promise<void> {
  const res = await apiFetch(`${BASE}/paw/infra/railway/redeploy`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify({ image_tag: imageTag ?? null }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `Update failed: ${res.status}`);
  }
}

// ──────────────────── Chat / Session API ────────────────────

export async function createSession(params: {
  agent_id: string;
  user_message: string;
  system_prompt?: string;
}): Promise<{ session_id: string }> {
  // Create session entity
  const res = await apiFetch(`${BASE}/tdata/Sessions`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify({}),
  });
  if (!res.ok) throw new Error(`Create session failed: ${res.status}`);
  const data = await res.json();
  const sessionId = data.entity_id || data.fields?.Id || data.Id;

  // Configure it — which kicks off the WASM-driven loop
  const configRes = await apiFetch(`${BASE}/tdata/Sessions('${sessionId}')/TemperPaw.Configure`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify({
      agent_id: params.agent_id,
      user_message: params.user_message,
      system_prompt: params.system_prompt || '',
    }),
  });
  if (!configRes.ok) throw new Error(`Configure session failed: ${configRes.status}`);

  return { session_id: sessionId };
}

export async function steerSession(sessionId: string, message: string): Promise<void> {
  const res = await apiFetch(`${BASE}/tdata/Sessions('${sessionId}')/TemperPaw.Steer`, {
    method: 'POST',
    headers: { ...HEADERS, 'content-type': 'application/json' },
    body: JSON.stringify({
      steering_messages: JSON.stringify([{ role: 'user', content: message }]),
    }),
  });
  if (!res.ok) throw new Error(`Steer session failed: ${res.status}`);
}
