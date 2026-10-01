/** Parse the Foresight API at the boundary. Missing measurements stay missing. */
type Row = Record<string, unknown>;
function field(row: Row, name: string): unknown {
  const key = Object.keys(row).find((key) => key.replaceAll('_', '').toLowerCase() === name.replaceAll('_', '').toLowerCase());
  return key ? row[key] : undefined;
}
function text(row: Row, name: string): string {
  const value = field(row, name);
  return typeof value === 'string' ? value : '';
}
function number(value: unknown): number | null {
  if ((typeof value !== 'number' && typeof value !== 'string') || (typeof value === 'string' && value.trim() === '')) return null;
  const result = Number(value);
  return Number.isFinite(result) ? result : null;
}
export function probability(value: unknown): number | null {
  const result = number(value);
  return result !== null && result >= 0 && result <= 1 ? result : null;
}
function record(value: unknown): Row | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Row : null;
}
function json(value: string): unknown { return JSON.parse(value); }
function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
}
function identity(row: Row) {
  return { id: text(row, 'Id') || text(row, '_entity_id'), status: text(row, 'Status') };
}
export interface PredictiveModel { version: string; slope: number; intercept: number }
function parseModel(value: string): PredictiveModel | null {
  if (!value) return null;
  try {
    const model = record(json(value));
    if (!model) return null;
    const slope = number(model.slope), intercept = number(model.intercept);
    return typeof model.version === 'string' && slope !== null && intercept !== null ? { version: model.version, slope, intercept } : null;
  } catch { return null; }
}
export function parseWorld(row: Row) {
  return {
    ...identity(row), title: text(row, 'name') || text(row, 'domain') || text(row, 'Id'),
    domain: text(row, 'domain'), description: text(row, 'description'), target: text(row, 'target_date'), frontier: text(row, 'frontier_date'),
    axes: text(row, 'uncertainty_axes'), mode: text(row, 'learning_mode') || 'observed',
    model: parseModel(text(row, 'model_json')), adoptedRunId: text(row, 'adopted_learning_run_id'),
    error: text(row, 'error_message'), corpusId: text(row, 'corpus_file_id'),
    agentProvider: text(row, 'agent_provider'), agentModel: text(row, 'agent_model'),
    researchSessionId: text(row, 'research_session_id'),
    explorationPhase: text(row, 'exploration_phase'),
  };
}
export type World = ReturnType<typeof parseWorld>;
export type ResearchSessionState =
  | { kind: 'unlinked' }
  | { kind: 'loading'; id: string }
  | { kind: 'unavailable'; id: string; error: string }
  | { kind: 'ready'; id: string; status: string; error: string; turns: number | null; heartbeat: string };

export async function loadResearchSession(
  id: string,
  fetchEntity: (set: string, id: string) => Promise<unknown>,
): Promise<ResearchSessionState> {
  if (!id) return { kind: 'unlinked' };
  try {
    const row = record(await fetchEntity('Sessions', id));
    if (!row || identity(row).id !== id || !identity(row).status) {
      throw new Error('The linked research session returned an incomplete or mismatched record.');
    }
    return { kind: 'ready', ...identity(row), error: text(row, 'error_message'),
      turns: number(field(row, 'turn_count')), heartbeat: text(row, 'last_heartbeat_at') };
  } catch (error) {
    return { kind: 'unavailable', id, error: error instanceof Error ? error.message : 'Could not load the linked research session.' };
  }
}

/** Only a verified failure belonging to this world can offer a research retry. */
export function canRetryResearch(
  world: World | null, session: ResearchSessionState, requestedSessionId?: string,
): boolean {
  return world?.status === 'Seeding' && session.kind === 'ready'
    && session.id === world.researchSessionId && session.id !== requestedSessionId
    && ['Failed', 'Cancelled'].includes(session.status);
}

/** Keep the old session fenced until the world reports its replacement, even after an uncertain HTTP failure. */
export async function retryForesightResearch(
  world: World, session: ResearchSessionState, requests: Record<string, string>,
  api: Pick<WorldOperations, 'postEntityAction'>,
): Promise<void> {
  if (session.kind !== 'ready' || !canRetryResearch(world, session, requests[world.id])) {
    throw new Error('Research retry requires a newly confirmed failed or cancelled session.');
  }
  requests[world.id] = session.id;
  try {
    await api.postEntityAction('Worlds', world.id, 'ResumeSeed', {});
  } catch (error) {
    // A client rejection did not accept the action; transport failures may have.
    if (error && typeof error === 'object' && 'status' in error
      && typeof error.status === 'number' && error.status >= 400 && error.status < 500 && error.status !== 408) {
      delete requests[world.id];
    }
    throw error;
  }
}

export function researchSessionProblem(session: ResearchSessionState, worldStatus: string): string {
  if (session.kind === 'unavailable') return `Research session ${session.id}: ${session.error}`;
  if (session.kind !== 'ready') return '';
  if (session.error) return `Research session ${session.status}: ${session.error}`;
  if (['Failed', 'Cancelled'].includes(session.status)) return `Research session ${session.status}; no error details were recorded.`;
  if (session.status === 'Completed' && worldStatus === 'Seeding') {
    return 'The last reported research session completed. The world is still Seeding; a retry may be starting or waiting to report its session.';
  }
  return '';
}

export function parseForecast(row: Row) {
  return {
    ...identity(row), error:text(row,'error_message'), eventId: text(row, 'event_node_id'), worldId: text(row, 'world_id'),
    question: text(row, 'question'), probability: probability(field(row, 'probability')),
    baseProbability: probability(field(row, 'base_probability')), registered: text(row, 'registered_at'),
    deadline: text(row, 'resolve_by'), outcome: text(row, 'outcome'), resolved: text(row, 'resolved_at'),
    score: probability(field(row, 'brier')), sources: text(row, 'outcome_source_refs'),
    model: text(row, 'model_version') || text(row, 'engine_version'),
    learningRunId: text(row, 'learning_run_id'), previousId: text(row, 'previous_forecast_id'),
    evidence: text(row, 'evidence_kind'), outcomeEvidence: text(row, 'outcome_evidence_kind'), marketRef: text(row, 'market_ref'),
  };
}
export type Forecast = ReturnType<typeof parseForecast>;
export function forecastGroups(forecasts: Forecast[]) {
  const groups = new Map<string, Forecast[]>();
  for (const forecast of forecasts) {
    const key = forecast.eventId || forecast.id;
    groups.set(key, [...(groups.get(key) ?? []), forecast]);
  }
  return Array.from(groups, ([eventId, revisions]) => {
    revisions.sort((a, b) => b.registered.localeCompare(a.registered) || b.id.localeCompare(a.id));
    return { eventId, latest: revisions[0], revisions };
  }).sort((a, b) => b.latest.registered.localeCompare(a.latest.registered));
}
export function parseEvent(row: Row) {
  return { ...identity(row), statement: text(row, 'statement'), layer: text(row, 'layer'), date: text(row, 'resolve_by'),
    probability: probability(field(row, 'probability')), provenance: text(row, 'provenance'), sources: text(row, 'source_refs'),
    edges: text(row, 'edges'), resolution: text(row, 'resolution') };
}
export function parseEndpoint(row: Row) {
  return { ...identity(row), summary: text(row, 'summary'), bundleFileId:text(row,'bundle_file_id'), assumptions: text(row, 'driver_config'),
    weight: probability(field(row, 'weight')), error: text(row, 'error_message'), discardReason: text(row, 'discard_reason') };
}
export function parsePath(row: Row) {
  return { ...identity(row), endpointId: text(row, 'endpoint_id'), claimId:text(row,'claim_id'), classification: text(row, 'classification'),
    note: text(row, 'classification_note'), cost: number(field(row, 'repair_cost')), nodes: text(row, 'required_node_ids'),
    error: text(row, 'error_message') };
}
export function parseClaim(row: Row) {
  return { ...identity(row), endpointId: text(row, 'endpoint_id'), statement: text(row, 'current_text') || text(row, 'original_text'),
    classification: text(row, 'classification'), reason: text(row, 'unreachable_reason'), error: text(row, 'error_message') };
}
export interface LearningReport {
  decision: string; reason: string; mode: string; asOf: string;
  trainingCount: number | null; validationCount: number | null; skippedCount: number | null;
  incumbentBrier: number | null; candidateBrier: number | null; improvement: number | null;
  trainingIds: string[]; validationIds: string[]; limitations: string[];
  changes: { name: string; before: number | null; after: number | null; meaning: string }[];
  provenance: string; skipped: string;
}
function parseReport(value: string): LearningReport | null {
  if (!value) return null;
  const r = record(json(value));
  if (!r || r.schema_version !== 1 || typeof r.decision !== 'string' || typeof r.reason !== 'string') throw new Error('Unsupported learning report');
  const changes = Array.isArray(r.learned_changes) ? r.learned_changes.flatMap((value) => {
    const c = record(value);
    return c && typeof c.name === 'string' && typeof c.meaning === 'string'
      ? [{name:c.name, before:number(c.before), after:number(c.after), meaning:c.meaning}] : [];
  }) : [];
  return {
    decision:r.decision, reason:r.reason, mode:typeof r.mode === 'string' ? r.mode : '', asOf:typeof r.as_of === 'string' ? r.as_of : '',
    trainingCount:number(r.training_count), validationCount:number(r.validation_count), skippedCount:number(r.skipped_count),
    incumbentBrier:probability(r.incumbent_brier), candidateBrier:probability(r.candidate_brier), improvement:number(r.improvement),
    trainingIds:strings(r.training_ids), validationIds:strings(r.validation_ids), limitations:strings(r.evaluation_limitations),
    changes, provenance:typeof r.provenance_summary === 'string' ? r.provenance_summary : JSON.stringify(r.provenance_summary ?? {}),
    skipped:JSON.stringify(r.skipped_reasons ?? {}),
  };
}
export function parseLearningRun(row: Row) {
  let report: LearningReport | null = null, reportError = '';
  try { report = parseReport(text(row, 'report_json')); } catch { reportError = 'The learning report could not be read. Open the record to inspect the stored data.'; }
  return { ...identity(row), worldId:text(row,'world_id'), mode:text(row,'mode'), asOf:text(row,'as_of'),
    error:text(row,'error_message'), report, reportError, candidate:parseModel(text(row,'candidate_model_json')),
    incumbent:parseModel(text(row,'incumbent_model_json')) };
}
export type LearningRun = ReturnType<typeof parseLearningRun>;
export function sourceLinks(value: string): string[] {
  let candidates = [value];
  try { candidates = strings(json(value)); } catch { /* A single URL is also a valid source reference. */ }
  return candidates.filter((candidate) => { try { const u = new URL(candidate); return u.protocol === 'https:' || u.protocol === 'http:'; } catch { return false; } });
}
export function parseDataset(value: string): string {
  if (!value.trim()) return '';
  let parsed: unknown;
  try { parsed = json(value); } catch { throw new Error('Dataset must be valid JSON.'); }
  if (!Array.isArray(parsed)) throw new Error('Dataset must be a JSON array.');
  if (parsed.length > 512) throw new Error('Dataset is limited to 512 experiences.');
  if (value.length > 1_000_000) throw new Error('Dataset exceeds the 1 MB limit.');
  return JSON.stringify(parsed);
}
export function percent(value: number | null): string { return value === null ? 'Unmeasured' : `${(value * 100).toFixed(1)}%`; }
export function measure(value: number | null): string { return value === null ? '—' : value.toFixed(4); }

/** Convert UI date inputs to the backend's fixed UTC-second action format. */
export function utcTime(value: string): string {
  const parsed = new Date(value.endsWith('Z') ? value : value + 'Z');
  if (!Number.isFinite(parsed.getTime())) throw new Error('Enter a valid UTC date and time.');
  return parsed.toISOString().slice(0, 19) + 'Z';
}

/** Start a new outcome draft at the world's current clock, never a prior form's date. */
export function outcomeFormDefaults(mode: string, replayClock: string, now = new Date()) {
  const instant = mode === 'observed' ? now.toISOString() : replayClock;
  return {time:utcTime(instant).slice(0, 19), outcome:'yes', sources:''};
}

export interface ForesightLocation { worldId: string; asOf: string; tab: string }
export function readForesightLocation(href: string): ForesightLocation {
  const params = new URL(href).searchParams;
  const at = params.get('at') ?? '';
  const date = new Date(at + 'Z');
  const asOf = Number.isFinite(date.getTime()) && date.toISOString().slice(0, 16) === at ? at : '';
  const view = params.get('view') ?? '';
  return { worldId: params.get('world') ?? '', asOf,
    tab: ['world','predictions','learning','activity'].includes(view) ? view : 'world' };
}
export function writeForesightLocation(href: string, state: ForesightLocation): string {
  const url = new URL(href);
  for (const [key, value] of Object.entries({world:state.worldId, at:state.asOf, view:state.tab})) {
    if (value) url.searchParams.set(key, value);
    else url.searchParams.delete(key);
  }
  return url.href;
}

/** Registration excludes certain/impossible claims, so validate before creating a question. */
export function predictionInputProbability(percent: number): number {
  if (!Number.isFinite(percent) || percent <= 0 || percent >= 100) {
    throw new Error('Use a probability greater than 0 and less than 100 percent.');
  }
  return percent / 100;
}

export type ResearchConfiguration =
  | { ready: true; provider: string; model: string }
  | { ready: false; reason: string };

/** Use credential names only; credentials remain in the server's vault. */
export function researchConfiguration(providerValue: unknown, modelValue: unknown, keysValue: unknown): ResearchConfiguration {
  const providerName = typeof providerValue === 'string' ? providerValue.trim().toLowerCase() : '';
  const model = typeof modelValue === 'string' ? modelValue.trim() : '';
  const aliases: Record<string, string> = {
    codex:'openai_codex', 'openai-codex':'openai_codex', open_router:'openrouter',
    hf:'huggingface', hugging_face:'huggingface', 'hugging-face':'huggingface',
    fireworks_ai:'fireworks', 'fireworks-ai':'fireworks', sakana:'sakana_fugu',
    'sakana-fugu':'sakana_fugu', fugu:'sakana_fugu', ollama:'local_openai',
    local:'local_openai', 'local-openai':'local_openai', 'openai-compatible':'openai_compatible',
    openai_compat:'openai_compatible', 'openai-compat':'openai_compatible', custom_openai:'openai_compatible',
  };
  const provider = aliases[providerName] ?? providerName;
  if (!provider || !model) return {ready:false, reason:'Configure a research provider and model in Settings.'};
  const keys = new Set(strings(keysValue));
  const requirements: Record<string, string[][]> = {
    anthropic:[['anthropic_api_key']], openai:[['openai_api_key']],
    openai_codex:[['openai_codex_access_token','openai_codex_token']],
    openrouter:[['openrouter_api_key']], huggingface:[['huggingface_api_key','hf_token']],
    fireworks:[['fireworks_api_key']], sakana_fugu:[['sakana_fugu_api_key'],['sakana_fugu_api_url']],
    local_openai:[['local_openai_api_url']], openai_compatible:[['openai_compatible_api_url']],
  };
  const required = requirements[provider];
  if (!required) return {ready:false, reason:`Configure a supported research provider in Settings; “${provider}” is not supported.`};
  if (!required.every(alternatives => alternatives.some(key => keys.has(key)))) {
    return {ready:false, reason:`Configure ${provider} access in Settings before starting research.`};
  }
  return {ready:true, provider, model};
}

interface WorldOperations {
  createEntity(set: string): Promise<Record<string, unknown>>;
  postEntityAction(set: string, id: string, action: string, body?: Record<string, unknown>): Promise<unknown>;
}
interface WorldCreation {
  name: string; domain: string; description: string; target: string;
  mode: string; budget: number; asOf: string;
}
function researchFields(configuration: ResearchConfiguration): {agent_provider:string; agent_model:string} {
  if (!configuration.ready) throw new Error(configuration.reason);
  return {agent_provider:configuration.provider, agent_model:configuration.model};
}

/** Validate configuration before creating a durable world. Seed remains an explicit user action. */
export async function createForesightWorld(input: WorldCreation, research: ResearchConfiguration, api: WorldOperations): Promise<string> {
  const agent = input.mode === 'observed' ? researchFields(research) : {};
  const target = utcTime(input.target);
  const replayClock = input.mode === 'observed' ? '' : utcTime(input.asOf);
  const created = await api.createEntity('Worlds');
  const id = text(created, 'Id') || text(created, '_entity_id');
  if (!id) throw new Error('The world was created without a returned identifier.');
  await api.postEntityAction('Worlds', id, 'ConfigureLearning', {learning_mode:input.mode});
  await api.postEntityAction('Worlds', id, 'Configure', {
    name:input.name.trim(), domain:input.domain.trim(), description:input.description.trim(),
    target_date:target, frontier_date:target, horizon_months:'12', endpoint_budget:'3',
    token_budget_cents:String(input.budget), hindcast_mode:input.mode === 'historical' ? 'true' : 'false',
    ...agent,
  });
  if (input.mode !== 'observed') {
    await api.postEntityAction('Worlds', id, 'OpenReplay', {
      learning_mode:input.mode, domain:input.domain.trim(), frontier_date:target, last_ingest_date:replayClock,
    });
  }
  return id;
}

export async function startForesightResearch(id: string, research: ResearchConfiguration, api: WorldOperations): Promise<void> {
  await api.postEntityAction('Worlds', id, 'Configure', researchFields(research));
  await api.postEntityAction('Worlds', id, 'Seed', {});
}

/** Weighted futures are complete; other settled states remain visible separately. */
export function futureProgress(endpoints: ReturnType<typeof parseEndpoint>[]) {
  const counts = {pending:0, completed:0, discarded:0, failed:0};
  for (const endpoint of endpoints) {
    if (endpoint.status === 'Weighted') counts.completed++;
    else if (endpoint.status === 'Discarded') counts.discarded++;
    else if (endpoint.status === 'Failed') counts.failed++;
    else counts.pending++;
  }
  return counts;
}

export function explorationMessage(world: Pick<World, 'status' | 'explorationPhase'>, hasFutures: boolean): string {
  if (world.status === 'Failed') return 'World processing failed. Saved futures and predictions remain available; see the error below.';
  if (hasFutures && world.explorationPhase === 'first_pass') return 'First pass: evaluating up to three key claims per future. Accepted paths publish predictions as they finish.';
  if (world.explorationPhase === 'deepening') return 'First pass ready. The engine is exploring additional paths in the background; existing predictions remain available.';
  if (world.explorationPhase === 'complete') return 'This exploration pass is complete. Predictions remain open for evidence and outcomes.';
  return '';
}

/** Sampling reuses existing endpoints without resetting them, so expose only the first pass. */
export async function startForesightExploration(
  world: World, endpoints: ReturnType<typeof parseEndpoint>[],
  api: Pick<WorldOperations, 'postEntityAction'>,
): Promise<void> {
  if (world.status !== 'Active') throw new Error('Exploration requires an active world.');
  if (!world.agentProvider || !world.agentModel) throw new Error('This world needs a research provider and model before exploration.');
  if (endpoints.length) throw new Error('This world already has possible futures. Inspect their progress before continuing.');
  await api.postEntityAction('Worlds', world.id, 'SampleEndpoints', {});
}

/** This is a requirement list, not a derived causal or chronological ordering. */
export function requiredEvents(value: string, events: ReturnType<typeof parseEvent>[]) {
  let ids: unknown;
  try { ids = json(value || '[]'); }
  catch { return {items:[], error:'The route has an unreadable required-event list.'}; }
  if (!Array.isArray(ids) || !ids.every((id): id is string => typeof id === 'string' && id.trim().length > 0)) {
    return {items:[], error:'The route has an invalid required-event list.'};
  }
  const byId = new Map(events.map(event => [event.id, event]));
  return {items:ids.map(id => ({id, event:byId.get(id) ?? null})), error:''};
}

export type FutureBundleState =
  | {kind:'loading'}
  | {kind:'ready'; text:string}
  | {kind:'unavailable'; error:string};

/** Read the saved document through the same authenticated API boundary as entity reads. */
export async function loadFutureBundle(
  fileId: string, fetchResponse: (path: string) => Promise<Response>,
): Promise<FutureBundleState> {
  try {
    const quotedId = encodeURIComponent(fileId.replaceAll("'", "''"));
    const response = await fetchResponse(`/tdata/Files('${quotedId}')/$value`);
    if (!response.ok) throw new Error(`The saved future story could not be read (HTTP ${response.status}).`);
    return {kind:'ready', text:await response.text()};
  } catch (error) {
    return {kind:'unavailable', error:error instanceof Error ? error.message : 'The saved future story could not be read.'};
  }
}
