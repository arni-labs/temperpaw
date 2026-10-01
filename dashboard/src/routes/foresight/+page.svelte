<script lang="ts">
  import { onMount } from 'svelte';
  import simulatedExamples from '../../../../os-apps/paw-foresight/fixtures/simulated-calibration.json';
  import { base } from '$app/paths';
  import { replaceState } from '$app/navigation';
  import { page } from '$app/stores';
  import { apiFetch, createEntity, postEntityAction, queryEntities, getEntity, fetchSetupStatus, getSecret, listSecretKeys } from '$lib/api';
  import { createSSEConnection, type StateChangeEvent } from '$lib/sse';
  import { parseWorld, parseForecast, parseEvent, parseEndpoint, parsePath, parseClaim, parseLearningRun,
    loadResearchSession, researchSessionProblem, canRetryResearch, retryForesightResearch, type ResearchSessionState, forecastGroups, sourceLinks, parseDataset, percent, measure, utcTime, readForesightLocation, writeForesightLocation, predictionInputProbability, outcomeFormDefaults, researchConfiguration, createForesightWorld, startForesightResearch, startForesightExploration, futureProgress, explorationMessage, requiredEvents, loadFutureBundle, type FutureBundleState, type ResearchConfiguration, type World, type Forecast, type LearningRun } from '$lib/foresight';

  let worlds = $state<World[]>([]);
  let selectedId = $state('');
  let locationReady = $state(false);
  let loading = $state(false);
  let busy = $state(false);
  let errors = $state<string[]>([]);
  let notices = $state<string[]>([]);
  let message = $state('');
  let actionError = $state('');
  let streamStatus = $state('Connecting');
  let tab = $state('world');
  let forecasts = $state<Forecast[]>([]);
  let events = $state<ReturnType<typeof parseEvent>[]>([]);
  let endpoints = $state<ReturnType<typeof parseEndpoint>[]>([]);
  let endpointsAvailable = $state(false);
  let explorationRequests = $state<Record<string, boolean>>({});
  let futureBundles = $state<Record<string, FutureBundleState>>({});
  let paths = $state<ReturnType<typeof parsePath>[]>([]);
  let claims = $state<ReturnType<typeof parseClaim>[]>([]);
  let runs = $state<LearningRun[]>([]);
  let activity = $state<StateChangeEvent[]>([]);
  let researchSession = $state<ResearchSessionState>({kind:'unlinked'});
  let researchRetryRequests = $state<Record<string, string>>({});
  let showCreate = $state(false);
  let newName = $state('');
  let newDomain = $state('');
  let newDescription = $state('');
  let newTarget = $state('');
  let newMode = $state('observed');
  let newBudget = $state(100);
  let researchLoading = $state(true);
  let researchKeys = $state<string[]>([]);
  let configuredResearch = $state<ResearchConfiguration>({ready:false, reason:'Checking research configuration…'});
  let asOf = $state(new Date().toISOString().slice(0, 16));
  let dataset = $state('');
  let showQuestion = $state(false);
  let questionText = $state('');
  let questionProbability = $state(55);
  let questionDeadline = $state('');
  let questionSources = $state('');
  let outcomeForecast = $state('');
  let outcome = $state('yes');
  let outcomeSources = $state('');
  let outcomeTime = $state('');
  let loadGeneration = 0;
  let disposed = false;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;

  const world = $derived(worlds.find((item) => item.id === selectedId) ?? null);
  const groupedForecasts = $derived(forecastGroups(forecasts));
  const futures = $derived(futureProgress(endpoints));
  const explorationProgress = $derived(world ? explorationMessage(world, endpoints.length > 0) : '');
  const explorationRequested = $derived(explorationRequests[selectedId] === true);
  const researchProblem = $derived(researchSessionProblem(researchSession, world?.status ?? ''));
  const researchRetryEligible = $derived(canRetryResearch(world, researchSession));
  const researchRetryPending = $derived(Boolean(world && world.status === 'Seeding'
    && researchRetryRequests[world.id] === world.researchSessionId));
  const failures = $derived([
    ...(researchProblem ? [researchProblem] : []),
    ...(world?.error ? [world.error] : []),
    ...endpoints.flatMap((item) => item.error ? [item.error] : []),
    ...claims.flatMap((item) => item.error ? [`Claim ${item.id}: ${item.error}`] : []),
    ...paths.flatMap((item) => item.error ? [item.error] : []),
    ...forecasts.flatMap((item) => item.error ? [item.error] : []),
    ...runs.flatMap((item) => item.error ? [item.error] : []),
  ]);
  const busyRun = $derived(runs.some((run) => ['Preparing','Training','Evaluating','Adopting'].includes(run.status)));
  const sortedRuns = $derived([...runs].sort((a, b) => b.id.localeCompare(a.id)));
  const activeModel = $derived(world?.model?.version || 'Uncalibrated baseline');
  const worldResearch = $derived(world?.agentProvider && world.agentModel
    ? researchConfiguration(world.agentProvider, world.agentModel, researchKeys) : configuredResearch);

  $effect(() => {
    if (!locationReady) return;
    const href = writeForesightLocation(window.location.href, {worldId:selectedId, asOf, tab});
    if (href !== window.location.href) replaceState(href, $page.state);
  });

  function recordHref(type: string, id: string): string { return `${base}/entities/${type}s/${encodeURIComponent(id)}`; }
  function date(value: string): string { return value ? value.replace('T', ' ').replace(/\.\d+Z$/, ' UTC').replace(/Z$/, ' UTC') : 'Not recorded'; }
  function escape(value: string): string { return value.replaceAll("'", "''"); }
  async function loadWorlds() {
    const rows = await queryEntities('Worlds', undefined, 'Id desc', 101);
    if (disposed) return;
    worlds = rows.slice(0, 100).map(parseWorld);
    if (rows.length > 100) notices = [...notices.filter((value) => !value.startsWith('World list')), 'World list shows the newest 100 worlds.'];
    if (!selectedId && worlds.length) selectedId = worlds[0].id;
  }
  async function load() {
    const generation = ++loadGeneration;
    loading = true;
    endpointsAvailable = false;
    errors = [];
    notices = [];
    try { await loadWorlds(); } catch (err) { errors = [err instanceof Error ? err.message : 'Could not load worlds.']; }
    if (!selectedId || disposed || generation !== loadGeneration) { loading = false; return; }
    const filter = `world_id eq '${escape(selectedId)}'`;
    const sets = ['EventNodes','Endpoints','Paths','Claims','Forecasts','LearningRuns'];
    const researchSessionId = worlds.find((item) => item.id === selectedId)?.researchSessionId ?? '';
    researchSession = researchSessionId ? {kind:'loading', id:researchSessionId} : {kind:'unlinked'};
    const [results, session] = await Promise.all([
      Promise.allSettled(sets.map((set) => queryEntities(set, filter, 'Id desc', 501))),
      loadResearchSession(researchSessionId, getEntity),
    ]);
    if (disposed || generation !== loadGeneration) return;
    researchSession = session;
    const rows = results.map((result, index) => {
      if (result.status === 'rejected') {
        errors.push(`${sets[index]}: ${result.reason instanceof Error ? result.reason.message : 'Could not load records'}`);
        return [];
      }
      if (result.value.length > 500) notices.push(`${sets[index]}: showing the newest 500 records. Older records are excluded from this view.`);
      return result.value.slice(0, 500);
    });
    events = rows[0].map(parseEvent); endpoints = rows[1].map(parseEndpoint);
    const endpointResult = results[1];
    endpointsAvailable = endpointResult.status === 'fulfilled' && endpointResult.value.length <= 500;
    if (endpoints.length || ['Failed','Archived'].includes(world?.status ?? '')) delete explorationRequests[selectedId];
    paths = rows[2].map(parsePath); claims = rows[3].map(parseClaim);
    forecasts = rows[4].map(parseForecast); runs = rows[5].map(parseLearningRun);
    loading = false;
  }
  async function chooseWorld() {
    dataset = ''; outcomeForecast = ''; message = ''; activity = []; researchSession = {kind:'unlinked'};
    forecasts = []; events = []; endpoints = []; paths = []; claims = []; runs = [];
    await load();
  }
  async function perform(action: () => Promise<void>) {
    if (busy) return;
    busy = true; message = ''; actionError = '';
    try { await action(); }
    catch (err) { actionError = err instanceof Error ? err.message : 'The action failed.'; }
    finally { busy = false; await load(); }
  }
  async function loadResearchConfiguration(): Promise<ResearchConfiguration> {
    researchLoading = true;
    try {
      const [setup, model, keys] = await Promise.all([fetchSetupStatus(), getSecret('llm_model'), listSecretKeys()]);
      researchKeys = keys;
      configuredResearch = researchConfiguration(setup.llm_provider, model, keys);
    } catch (err) {
      researchKeys = [];
      configuredResearch = {ready:false, reason:err instanceof Error ? err.message : 'Could not read research configuration.'};
    } finally { researchLoading = false; }
    return configuredResearch;
  }
  async function createWorld() {
    await perform(async () => {
      const research = newMode === 'observed' ? await loadResearchConfiguration() : configuredResearch;
      selectedId = await createForesightWorld({
        name:newName, domain:newDomain, description:newDescription, target:newTarget,
        mode:newMode, budget:newBudget, asOf,
      }, research, {createEntity, postEntityAction});
      message = newMode === 'observed' ? 'World configured. Start research when you are ready.' : 'Replay world opened. Add dated experiences in “What it learned”.';
      if (newMode !== 'observed') tab = 'learning';
      showCreate = false;
    });
  }
  async function worldAction(action: string) {
    await perform(async () => {
      if (action === 'Seed') {
        await loadResearchConfiguration();
        await startForesightResearch(selectedId, worldResearch, {createEntity, postEntityAction});
      } else {
        await postEntityAction('Worlds', selectedId, action, action === 'RegisterForecasts' ? {last_ingest_date:world?.mode === 'observed' ? utcTime(new Date().toISOString()) : utcTime(asOf)} : {});
      }
      message = action === 'RegisterForecasts' ? 'Prediction registration requested.' : 'World research started.';
    });
  }
  async function retryResearch() {
    if (!world || busy || loading || !canRetryResearch(world, researchSession, researchRetryRequests[world.id])) return;
    const selectedWorld = world;
    const failedSession = researchSession;
    await perform(async () => {
      await retryForesightResearch(selectedWorld, failedSession, researchRetryRequests, {postEntityAction});
      message = 'Research retry accepted. Waiting for its new session.';
    });
  }
  async function exploreFutures() {
    if (!world || busy || loading || !endpointsAvailable || explorationRequested) return;
    const selectedWorld = world;
    await perform(async () => {
      if (!worldResearch.ready) throw new Error(worldResearch.reason);
      explorationRequests[selectedWorld.id] = true;
      try {
        await startForesightExploration(selectedWorld, endpoints, {postEntityAction});
      } catch (err) {
        delete explorationRequests[selectedWorld.id];
        throw err;
      }
      if (selectedId === selectedWorld.id) {
        message = 'Exploration requested. Future records will show its progress below.';
        tab = 'world';
      }
    });
  }
  async function readFutureBundle(fileId: string) {
    const existing = futureBundles[fileId];
    if (existing?.kind === 'loading' || existing?.kind === 'ready') return;
    futureBundles[fileId] = {kind:'loading'};
    const result = await loadFutureBundle(fileId, apiFetch);
    if (!disposed) futureBundles[fileId] = result;
  }
  async function startLearning() {
    await perform(async () => {
      if (!world) throw new Error('Choose a world first.');
      const prepared = parseDataset(dataset);
      if (world.mode === 'observed' && prepared) throw new Error('Observed learning uses recorded outcomes. Choose a historical or simulated world to supply replay data.');
      if (world.mode !== 'observed' && !prepared) throw new Error('Supply a dated replay dataset.');
      const run = await createEntity('LearningRuns');
      const id = String(run.Id ?? run._entity_id ?? '');
      if (!id) throw new Error('The learning run has no identifier.');
      await postEntityAction('LearningRuns', id, 'Start', { world_id:selectedId, mode:world.mode, as_of:utcTime(asOf), dataset_json:prepared });
      message = 'Learning requested. Its progress and evaluation will appear below.';
    });
  }
  async function addQuestion() {
    await perform(async () => {
      if (!questionText.trim()) throw new Error('Enter a prediction question.');
      const inputProbability = predictionInputProbability(questionProbability);
      const refs = questionSources.split('\n').map((value) => value.trim()).filter(Boolean);
      if (!refs.length) throw new Error('Include the evidence or fixture supporting this question.');
      const registeredAt = world?.mode === 'observed' ? utcTime(new Date().toISOString()) : utcTime(asOf);
      const deadline = utcTime(questionDeadline);
      if (deadline <= registeredAt) throw new Error('The question must resolve after the prediction time.');
      await createEntity('EventNodes', {
        world_id:selectedId, statement:questionText.trim(), probability:String(inputProbability),
        provenance:'authored', source_refs:JSON.stringify(refs), resolve_by:deadline, layer:'fast',
        author_agent_id:'dashboard',
      });
      await postEntityAction('Worlds', selectedId, 'RegisterForecasts', {last_ingest_date:registeredAt});
      message = 'Question added and prediction registration requested.';
      showQuestion = false;
    });
  }
  function openOutcome(id: string) {
    if (!world) return;
    try {
      const draft = outcomeFormDefaults(world.mode, asOf);
      outcomeTime = draft.time;
      outcome = draft.outcome;
      outcomeSources = draft.sources;
      outcomeForecast = id;
      actionError = '';
    } catch (err) {
      actionError = err instanceof Error ? err.message : 'Could not prepare the outcome form.';
    }
  }
  async function recordOutcome() {
    await perform(async () => {
      const refs = outcomeSources.split('\n').map((source) => source.trim()).filter(Boolean);
      if (!refs.length) throw new Error('Include at least one outcome source.');
      await postEntityAction('Forecasts', outcomeForecast, 'RecordOutcome', {
        outcome, outcome_source_refs:JSON.stringify(refs), resolved_at:utcTime(outcomeTime),
      });
      message = 'Outcome submitted for scoring and learning.';
      outcomeForecast = '';
    });
  }
  onMount(() => {
    const saved = readForesightLocation(window.location.href);
    selectedId = saved.worldId;
    if (saved.asOf) asOf = saved.asOf;
    tab = saved.tab;
    locationReady = true;
    void load();
    void loadResearchConfiguration();
    const stream = createSSEConnection('foresight', undefined, undefined, (event) => {
      if (event.entity_type === 'Session') {
        if (event.entity_id !== world?.researchSessionId) return;
      } else if (!['World','EventNode','Endpoint','Path','Claim','Forecast','LearningRun'].includes(event.entity_type)) return;
      const known = [selectedId, world?.researchSessionId, ...events.map(e => e.id), ...endpoints.map(e => e.id), ...paths.map(e => e.id), ...forecasts.map(e => e.id), ...runs.map(e => e.id)];
      if (known.includes(event.entity_id)) activity = [event, ...activity].slice(0, 40);
      if (!refreshTimer) refreshTimer = setTimeout(() => { refreshTimer = undefined; void load(); }, 300);
    });
    stream.source.addEventListener('open', () => streamStatus = 'Live');
    stream.source.addEventListener('error', () => streamStatus = 'Reconnecting — refresh is available');
    return () => { disposed = true; loadGeneration++; clearTimeout(refreshTimer); stream.close(); };
  });
</script>

<svelte:head>
  <title>Foresight · TemperPaw</title>
  <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Cormorant+Garamond:wght@500;600&family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans:wght@400;500;600&display=swap" />
</svelte:head>

<div class="foresight">
  <header class="masthead">
    <div><p class="eyebrow">Temper / Foresight</p><h1>Watch the future take shape.</h1><p>Explore possible worlds, follow predictions, and see what experience changes.</p></div>
    <div class="live"><span class:connected={streamStatus === 'Live'}></span>{streamStatus}<button onclick={load} disabled={loading}>Refresh</button></div>
  </header>
  <div class="world-picker">
    <label>World<select bind:value={selectedId} onchange={chooseWorld} disabled={busy}><option value="">Choose a world</option>{#each worlds as item}<option value={item.id}>{item.title} · {item.status}</option>{/each}</select></label>
    <button class="primary" onclick={() => { showCreate = !showCreate; if (showCreate) void loadResearchConfiguration(); }}>{showCreate ? 'Close' : 'New world'}</button>
  </div>
  {#if showCreate}
    <form class="panel create-form" onsubmit={(e) => { e.preventDefault(); void createWorld(); }}>
      <h2>Create a world</h2>
      <label>Name<input bind:value={newName} required maxlength="150" placeholder="A future to investigate" /></label>
      <label>Domain<input bind:value={newDomain} required maxlength="300" placeholder="What changes are we exploring?" /></label>
      <label class="wide">Description<textarea bind:value={newDescription} rows="2" required placeholder="The situation, questions and assumptions."></textarea></label>
      <label>Target date<input type="date" bind:value={newTarget} required /></label>
      <label>Experience source<select bind:value={newMode}><option value="observed">Observed outcomes</option><option value="historical">Historical replay</option><option value="simulated">Simulated demonstration</option></select></label>
      {#if newMode !== 'observed'}<label>Replay starts at (UTC)<input type="datetime-local" bind:value={asOf} required /></label>{/if}
      {#if newMode === 'observed'}
        <div class="wide research-setup">
          {#if researchLoading}<p>Checking research configuration…</p>
          {:else if configuredResearch.ready}<p>Research model: <strong>{configuredResearch.model}</strong> · {configuredResearch.provider}</p>
          {:else}<p class="notice">{configuredResearch.reason}</p>{/if}
          <a href="{base}/settings">Configure research in Settings ↗</a>
          <button type="button" disabled={researchLoading} onclick={() => void loadResearchConfiguration()}>Refresh configuration</button>
        </div>
      {/if}
      <label>Research budget (cents)<input type="number" min="1" max="10000" bind:value={newBudget} required /></label>
      <div class="wide"><p class="small">Each world's evidence source stays distinct. Simulated learning demonstrates mechanics and does not establish real-world accuracy.</p><button class="primary" disabled={busy || (newMode === 'observed' && (researchLoading || !configuredResearch.ready))}>{busy ? 'Creating…' : 'Create world'}</button></div>
    </form>
  {/if}
  <div aria-live="polite">
    {#if message}<p class="notice">{message}</p>{/if}
    {#if actionError}<p class="error" role="alert">{actionError}</p>{/if}
    {#each errors as error}<p class="error" role="alert">{error}</p>{/each}
    {#each notices as notice}<p class="notice">{notice}</p>{/each}
  </div>
  {#if world}
    <section class="world-intro">
      <div><p class="eyebrow">{world.status} / {world.mode === 'simulated' ? 'Simulated demonstration' : world.mode === 'historical' ? 'Historical replay' : 'Observed evidence'}</p><h2>{world.title}</h2>{#if world.description}<p>{world.description}</p>{/if}<p>Target {world.target || 'not set'} · Prediction frontier {world.frontier || 'not set'}</p></div>
      <div class="world-actions">
        {#if world.status === 'Created'}
          {#if worldResearch.ready}<p class="small">Research model: {worldResearch.model} · {worldResearch.provider}</p>
          {:else}<p class="small">{worldResearch.reason}</p>{/if}
          <button class="primary" disabled={busy || researchLoading || !worldResearch.ready} onclick={() => worldAction('Seed')}>Start research</button>
          <a href="{base}/settings">Configure research ↗</a>
          <button disabled={researchLoading} onclick={() => void loadResearchConfiguration()}>Refresh configuration</button>
        {/if}
        {#if world.status === 'Active'}
          {#if !endpoints.length}
            <button class="primary" disabled={busy || loading || !endpointsAvailable || explorationRequested || researchLoading || !world.agentProvider || !world.agentModel || !worldResearch.ready} onclick={() => void exploreFutures()}>{explorationRequested ? 'Exploration requested…' : 'Explore possible futures'}</button>
            {#if !world.agentProvider || !world.agentModel}<p class="small">This world has no research model. Create an observed world with research configured to explore futures.</p>
            {:else if !researchLoading && !worldResearch.ready}<p class="small">{worldResearch.reason} <a href="{base}/settings">Configure research ↗</a></p><button disabled={researchLoading} onclick={() => void loadResearchConfiguration()}>Refresh configuration</button>{/if}
          {/if}
          <button disabled={busy} onclick={() => worldAction('RegisterForecasts')}>Update predictions</button>
        {/if}
        <a class="record-link" href={recordHref('World',world.id)}>World record ↗</a>
      </div>
    </section>
    {#if world.mode !== 'observed'}<label class="replay-clock">Replay clock (UTC)<input type="datetime-local" bind:value={asOf} required /><span class="small">New predictions use this time; learning uses only outcomes available by it.</span></label>{/if}
    <div class="metrics">
      <div><span>Events</span><strong>{events.length}</strong></div>
      <div><span>Possible futures</span><strong>{endpoints.length}</strong></div>
      <div><span>Predicted questions</span><strong>{groupedForecasts.length}</strong></div>
      <div><span>Learning runs</span><strong>{runs.length}</strong></div>
    </div>
    <nav class="view-nav" aria-label="Foresight views">
      {#each [['world','World'],['predictions','Predictions'],['learning','What it learned'],['activity','Activity']] as [key,label]}<button aria-pressed={tab === key} onclick={() => tab = key}>{label}{#if key === 'activity' && failures.length}<span class="badge">{failures.length}</span>{/if}</button>{/each}
    </nav>

    {#if tab === 'world'}
      <section class="panel"><p class="eyebrow">The situation</p><h2>Events and dependencies</h2><p class="small">Evidence describes what is known; predictions describe what may happen. Follow an event's dependencies to inspect its assumptions.</p>
        {#if !events.length}<p class="empty">{loading ? 'Loading events…' : 'No events have been recorded for this world yet.'}</p>{/if}
        <ol class="event-list">{#each events as event}<li><div class="event-title"><h3>{event.statement || 'Untitled event'}</h3><span class="probability">{percent(event.probability)}</span></div><p class="small">{event.layer || 'Unclassified'} · {event.status} · {date(event.date)}</p>
          {#if event.resolution}<p>Outcome: {event.resolution}</p>{/if}
          <details><summary>Evidence and dependencies</summary><p>{event.provenance || 'No provenance recorded.'}</p><pre>{event.sources || 'No source references recorded.'}</pre>{#each sourceLinks(event.sources) as link}<a href={link} target="_blank" rel="noreferrer">{link} ↗</a>{/each}<p class="small">Dependencies</p><pre>{event.edges || 'None recorded.'}</pre><a href={recordHref('EventNode',event.id)}>Open event record ↗</a></details>
        </li>{/each}</ol>
      </section>
      <section class="panel"><p class="eyebrow">Alternative futures</p><h2>Where this world could go</h2><p class="small">Route cost measures how difficult a scenario is to support. It is separate from the probability of a prediction.</p>
        {#if loading}<p class="small">Checking future records…</p>
        {:else if !endpointsAvailable}<p class="notice">The complete future list is unavailable. Exploration cannot be started until it can be checked.</p>
        {:else if endpoints.length}<p aria-live="polite">{futures.completed} completed · {futures.pending} in progress · {futures.discarded} discarded · {futures.failed} failed</p>
        {:else if explorationRequested}<p class="notice" role="status">Exploration was requested. Waiting for the first future record.</p>
        {:else}<p class="empty">No alternative futures yet. Use “Explore possible futures” to develop scenarios from this world's research.</p>{/if}
        <div class="future-grid">{#each endpoints as endpoint}
          {@const bundle = futureBundles[endpoint.bundleFileId]}
          <article class="future"><p class="eyebrow">{endpoint.status}</p><h3>{endpoint.summary || 'Future being developed'}</h3>
            {#if endpoint.error}<p class="error">{endpoint.error}</p>{/if}
            {#if endpoint.discardReason}<p>{endpoint.discardReason}</p>{/if}
            <a href={recordHref('Endpoint',endpoint.id)}>Future record ↗</a>
            {#if endpoint.bundleFileId}
              <details ontoggle={(event) => { if (event.currentTarget.open) void readFutureBundle(endpoint.bundleFileId); }}>
                <summary>Read the full future story</summary>
                <p class="small">Scenario documents written from inside this possible future.</p>
                {#if bundle?.kind === 'ready'}<div class="future-story">{bundle.text || 'The saved story file is empty.'}</div>
                {:else if bundle?.kind === 'unavailable'}<p class="error" role="alert">{bundle.error}</p><button onclick={() => void readFutureBundle(endpoint.bundleFileId)}>Retry reading story</button>
                {:else}<p role="status">Loading the saved story…</p>{/if}
              </details>
            {:else}<p class="small">No future story file has been recorded yet.</p>{/if}
            <details><summary>Routes and requirements</summary>
              {#if !paths.some(path => path.endpointId === endpoint.id)}<p>No routes have been recorded for this future.</p>{/if}
              {#each paths.filter(path => path.endpointId === endpoint.id) as path}
                {@const requirements = requiredEvents(path.nodes, events)}
                {@const supportedClaim = claims.find(claim => claim.id === path.claimId)}
                <div class="route">
                  {#if supportedClaim}<p><strong>Supports:</strong> {supportedClaim.statement}</p>{/if}
                  <p><strong>{path.classification || path.status}</strong> · Repair cost {path.cost ?? 'unmeasured'}</p>
                  {#if path.note}<p>{path.note}</p>{/if}
                  {#if path.error}<p class="error">{path.error}</p>{/if}
                  <p><strong>Required events</strong></p><p class="small">Events this route requires, in recorded order.</p>
                  {#if requirements.error}<p class="error">{requirements.error}</p>
                  {:else if !requirements.items.length}<p class="small">No required events have been recorded for this route.</p>
                  {:else}<ul class="required-events">{#each requirements.items as requirement}
                    {@const prediction = groupedForecasts.find(group => group.eventId === requirement.id)?.latest}
                    <li><p>{requirement.event?.statement || 'Event not loaded in this view.'}</p>
                      {#if requirement.event}<p class="small">{date(requirement.event.date)} · Input probability {percent(requirement.event.probability)} · {requirement.event.status}</p>{/if}
                      <a href={recordHref('EventNode',requirement.id)}>Event record ↗</a>
                      {#if prediction}<a href={recordHref('Forecast',prediction.id)}>Prediction {percent(prediction.probability)} ↗</a>{/if}
                    </li>
                  {/each}</ul>{/if}
                  <a href={recordHref('Path',path.id)}>Route evidence ↗</a>
                </div>
              {/each}
              {#each claims.filter(claim => claim.endpointId === endpoint.id) as claim}<p><strong>{claim.classification || claim.status}:</strong> {claim.statement} {claim.reason}</p>{/each}
              <details><summary>Driver settings</summary><pre>{endpoint.assumptions || 'No driver assumptions recorded.'}</pre></details>
            </details>
          </article>
        {/each}</div>
      </section>
    {:else if tab === 'predictions'}
      <section class="panel"><p class="eyebrow">Beliefs with a record</p><h2>What the system expects</h2>{#if world.status === 'Active'}<button onclick={() => showQuestion = !showQuestion}>{showQuestion ? 'Close question form' : 'Add question'}</button>{/if}<p class="small">Earlier predictions remain visible when evidence or the model changes. Model in use: <strong>{activeModel}</strong>.</p>
        {#if !groupedForecasts.length}<p class="empty">No predictions registered yet.{#if world.status === 'Active'} {#if !endpoints.length && world.agentModel && world.agentProvider}Explore possible futures to develop scenarios, or add a question. {/if}Use “Update predictions” to register eligible events.{/if}</p>{/if}
        {#each groupedForecasts as group}<article class="prediction">
          <div class="event-title"><h3>{group.latest.question || 'Untitled prediction'}</h3><strong class="prediction-number">{percent(group.latest.probability)}</strong></div>
          <p class="small">{group.latest.status} · Resolves by {date(group.latest.deadline)} · {group.latest.evidence || 'Evidence kind not recorded'}</p>
          <p>Input {percent(group.latest.baseProbability)} → Prediction {percent(group.latest.probability)} <span class="small">/ {group.latest.model || 'Model not recorded'}</span></p>
          {#if group.latest.error}<p class="error">{group.latest.error}</p>{/if}
          {#if group.latest.outcome}<p><strong>Outcome: {group.latest.outcome}</strong> · {group.latest.outcomeEvidence || 'Outcome provenance not recorded'} · Score {measure(group.latest.score)} <span class="small">(Brier score; lower is better)</span></p>{/if}
          <div class="inline-actions"><a href={recordHref('Forecast',group.latest.id)}>Prediction record ↗</a>{#if group.latest.learningRunId}<a href={recordHref('LearningRun',group.latest.learningRunId)}>Learning behind this prediction ↗</a>{/if}{#if group.latest.status === 'Preregistered'}<button disabled={busy} onclick={() => openOutcome(group.latest.id)}>Record outcome</button>{/if}</div>
          <details><summary>{group.revisions.length} recorded {group.revisions.length === 1 ? 'prediction' : 'revisions'} · evidence and history</summary>{#each group.revisions as revision}<div class="revision"><p><strong>{percent(revision.probability)}</strong> · {date(revision.registered)} · {revision.model || 'Model unrecorded'}</p><p class="small">Input {percent(revision.baseProbability)} · {revision.status} · {revision.outcome || 'Unresolved'}{#if revision.outcome} · {revision.outcomeEvidence || 'Outcome provenance not recorded'}{/if}</p><pre>{revision.sources || revision.marketRef || 'No outcome evidence recorded.'}</pre>{#each sourceLinks(revision.sources || revision.marketRef) as link}<a href={link} target="_blank" rel="noreferrer">{link} ↗</a>{/each}<a class="small" href={recordHref('Forecast',revision.id)}>{revision.id}</a></div>{/each}</details>
        </article>{/each}
      </section>
      {#if showQuestion}<form class="panel" onsubmit={(e) => { e.preventDefault(); void addQuestion(); }}>
        <h2>A question to predict</h2><p class="small">State an event whose outcome can be verified. The adopted calibration transforms your input probability into a registered prediction.</p>
        <label>Question<textarea bind:value={questionText} required rows="2" placeholder="Will this event happen by the resolution date?"></textarea></label>
        <label>Input probability (%)<input type="number" min="0.1" max="99.9" step="0.1" bind:value={questionProbability} required /></label>
        <label>Resolves by (UTC)<input type="datetime-local" bind:value={questionDeadline} required /></label>
        <label>Evidence references, one per line<textarea bind:value={questionSources} rows="2" required placeholder={world.mode === 'simulated' ? 'fixture:foresight-calibration-mechanics-v1' : 'https://source.example/evidence'}></textarea></label>
        <button class="primary" disabled={busy}>Add and predict</button>
      </form>{/if}
      {#if outcomeForecast}<form class="panel outcome-form" onsubmit={(e) => { e.preventDefault(); void recordOutcome(); }}><h2>Record an outcome</h2><p>{forecasts.find(item => item.id === outcomeForecast)?.question}</p><label>Did the event happen?<select bind:value={outcome}><option value="yes">Yes</option><option value="no">No</option></select></label><label>Resolved at (UTC)<input type="datetime-local" step="1" bind:value={outcomeTime} required /></label><label>Sources, one per line<textarea bind:value={outcomeSources} required rows="3" placeholder="https://source.example/evidence"></textarea></label><div class="inline-actions"><button class="primary" disabled={busy}>Record and learn</button><button type="button" onclick={() => outcomeForecast = ''}>Cancel</button></div></form>{/if}
    {:else if tab === 'learning'}
      <section class="panel learning-intro"><p class="eyebrow">Learning from experience</p><h2>What changed, and why</h2><p>The learner adjusts how confidently it predicts events. Each candidate is tested against the previous model on the same held-out examples.</p><p class="small">This is learned calibration. It does not establish causal laws or guarantee future accuracy.</p><p class="small">Each run accepts up to 512 experiences. A model can use up to 512 events over its lifetime; a run that would exceed this limit fails visibly and keeps the previous model.</p><div class="model"><span>Current model</span><strong>{activeModel}</strong>{#if world.model}<span>Slope {measure(world.model.slope)} · Intercept {measure(world.model.intercept)}</span>{/if}</div>
        <form onsubmit={(e) => { e.preventDefault(); void startLearning(); }}>
          <label>Learn using evidence available by (UTC)<input type="datetime-local" bind:value={asOf} required /></label>
          {#if world.mode === 'simulated'}<div class="example-input"><button type="button" onclick={() => { dataset = JSON.stringify(simulatedExamples, null, 2); asOf = '2025-03-01T00:00'; }}>Load simulated examples</button><p class="small">36 fabricated, dated outcomes for demonstrating calibration. The engine computes the training and evaluation results when you run replay.</p></div>{/if}
          {#if world.mode !== 'observed'}<label>Dated {world.mode} experiences (JSON)<textarea bind:value={dataset} rows="7" spellcheck="false" placeholder="Paste a JSON array of dated experiences."></textarea></label><p class="small">Up to 512 experiences. Every item needs an event identity, input probability, outcome, dated registration and resolution, evidence kind and source references. Replay advances through dated evidence without waiting for real time.</p>{:else}<p class="small">This run uses verified outcomes already recorded in this world.</p>{/if}
          {#if world.mode === 'simulated'}<p class="notice">Simulated world: improvements here demonstrate the mechanism using synthetic evidence. They do not update an observed-world model.</p>{/if}
          <button class="primary" disabled={busy || busyRun || world.status !== 'Active'}>{busyRun ? 'Learning in progress…' : world.mode === 'observed' ? 'Learn from outcomes' : 'Run accelerated replay'}</button>
        </form>
      </section>
      {#if !runs.length}<p class="panel empty">No learning runs yet. The first run will record its data, changed parameters, evaluation and decision here.</p>{/if}
      {#each sortedRuns as run}<article class="panel learning-run">
        <div class="event-title"><div><p class="eyebrow">{run.mode || world.mode} / {date(run.asOf)}</p><h2>{run.status === 'Adopted' ? 'An improved calibration was adopted' : run.status === 'Rejected' ? 'The previous model was kept' : run.status === 'Failed' ? 'Learning needs attention' : run.status}</h2></div><a class="record-link" href={recordHref('LearningRun',run.id)}>Run record ↗</a></div>
        {#if run.error}<p class="error">{run.error}</p>{/if}{#if run.reportError}<p class="error">{run.reportError}</p>{/if}
        {#if run.report}
          <p>{run.report.reason}</p>
          <div class="comparison"><div><span>Previous model</span><strong>{measure(run.report.incumbentBrier)}</strong></div><div><span>Candidate model</span><strong>{measure(run.report.candidateBrier)}</strong></div><div><span>Held-out examples</span><strong>{run.report.validationCount ?? '—'}</strong></div></div>
          <p class="small">Brier score measures prediction error; lower is better. Trained on {run.report.trainingCount ?? 'unreported'} examples · Skipped {run.report.skippedCount ?? 'unreported'}.</p>
          {#each run.report.changes as change}<div class="change"><strong>{change.meaning}</strong><p class="small">{change.name}: {measure(change.before)} → {measure(change.after)}</p></div>{/each}
          {#each run.report.limitations as limitation}<p class="notice">{limitation}</p>{/each}
          <details><summary>Training and evaluation evidence</summary><p>{run.report.provenance}</p><p class="small">Training identities</p><pre>{run.report.trainingIds.join('\n') || 'None recorded'}</pre><p class="small">Held-out identities</p><pre>{run.report.validationIds.join('\n') || 'None recorded'}</pre><p class="small">Skipped evidence</p><pre>{run.report.skipped}</pre></details>
        {:else if !run.error && !run.reportError}<p class="small">The run has not published an evaluation yet. Current state: {run.status}.</p>{/if}
      </article>{/each}
    {:else}
      <section class="panel"><p class="eyebrow">Current work</p><h2>What is happening</h2><p>World: <strong>{world.status}</strong> · Live connection: {streamStatus}</p>
        <h3>Last recorded research session</h3>
        <p class="small">A retry may still be starting until its new session is reported here.</p>
        {#if researchSession.kind === 'ready'}
          <p><strong>{researchSession.status}</strong> · <a href={`${base}/sessions/${encodeURIComponent(researchSession.id)}`}>{researchSession.id}</a></p>
          <p class="small">Turns: {researchSession.turns ?? 'Not recorded'} · Last heartbeat: {date(researchSession.heartbeat)}</p>
        {:else if researchSession.kind === 'loading'}
          <p>Loading the linked research session…</p>
        {:else if researchSession.kind === 'unavailable'}
          <p>Research status could not be verified. <a href={`${base}/sessions/${encodeURIComponent(researchSession.id)}`}>Open the linked session</a>.</p>
        {:else if world.status === 'Created'}
          <p class="small">Research has not started for this world.</p>
        {:else if world.mode !== 'observed'}
          <p class="small">No research session is linked. Replay worlds can use supplied events without a research session.</p>
        {:else}
          <p class="error">No research session is linked to this world. Its research status and failures cannot be verified from this page.</p>
        {/if}
        {#if researchRetryEligible}
          <button onclick={retryResearch} disabled={busy || loading || researchRetryPending}>Retry research</button>
        {/if}
        {#if researchRetryPending}
          <p class="small" role="status">Waiting to verify a new research session. Refresh to check its status.</p>
        {/if}
        <h3>Possible futures</h3>
        {#if explorationProgress}<p>{explorationProgress}</p>{/if}
        {#if endpoints.length}
          <p>{futures.completed} completed · {futures.pending} in progress · {futures.discarded} discarded · {futures.failed} failed</p>
          {#each endpoints as endpoint}<p><strong>{endpoint.status}</strong> · <a href={recordHref('Endpoint',endpoint.id)}>{endpoint.summary || endpoint.id}</a></p>{/each}
        {:else if explorationRequested}<p>Exploration requested; waiting for the first future record.</p>
        {:else if loading}<p>Checking future records…</p>
        {:else if !endpointsAvailable}<p>Future progress could not be verified.</p>
        {:else}<p class="small">No exploration has been recorded for this world.</p>{/if}
        {#each runs.filter(run => ['Created','Preparing','Training','Evaluating','Adopting'].includes(run.status)) as run}<p><strong>{run.status}</strong> — Learning run <a href={recordHref('LearningRun',run.id)}>{run.id}</a></p>{/each}
        {#each failures as failure}<p class="error">{failure}</p>{/each}
        <h3>State changes observed in this session</h3>{#if !activity.length}<p class="empty">New changes will appear here while this page is open. Entity records preserve the full history.</p>{/if}
        <ol class="activity">{#each activity as event}<li><strong>{event.entity_type} · {event.action}</strong><span>{event.status}</span><a href={event.entity_type === 'Session' ? `${base}/sessions/${encodeURIComponent(event.entity_id)}` : recordHref(event.entity_type,event.entity_id)}>{event.entity_id}</a></li>{/each}</ol>
      </section>
    {/if}
  {:else if !loading}
    <section class="panel empty"><h2>A world to learn about.</h2><p>Create a world to explore its possible futures and track how evidence changes its predictions.</p></section>
  {:else}<p class="panel">Loading worlds…</p>{/if}
</div>

<style>
  .foresight { background:#fff; color:#101010; min-height:100vh; width:100%; padding:38px clamp(20px,4vw,64px) 66px; font:17px/1.55 'IBM Plex Sans',sans-serif; --rule:#1a1a1a; }
  .foresight :global(*) { box-sizing:border-box; }
  .foresight h1,.foresight h2,.foresight h3 { color:#101010; margin:0; font-weight:500; line-height:1.12; letter-spacing:-.02em; }
  .foresight h1 { font:500 clamp(38px,4vw,66px)/1.02 'Cormorant Garamond',serif; max-width:800px; }
  .foresight h2 { font:500 38px/1.1 'Cormorant Garamond',serif; }
  .foresight h3 { font:500 22px/1.35 'IBM Plex Sans',sans-serif; }
  .foresight p { margin:12px 0; }
  .eyebrow,.live,.metrics span,.model,.comparison span { font:13px/1.55 'IBM Plex Mono',monospace; }
  .eyebrow { text-transform:uppercase; letter-spacing:.08em; }
  .masthead { border-bottom:2px solid var(--rule); padding-bottom:29px; display:flex; gap:29px; justify-content:space-between; align-items:start; }
  .masthead p:not(.eyebrow) { max-width:600px; color:#6e6960; }
  .live { display:flex; flex-wrap:wrap; align-items:center; gap:8px; min-width:180px; }
  .live span { width:8px; height:8px; background:#ff3b1f; }.live .connected { background:#00aeef; }
  .world-picker { display:flex; gap:17px; align-items:end; padding:22px 0; }
  .world-picker label { flex:1; margin:0; }
  .foresight label { display:grid; gap:8px; margin-bottom:17px; font-size:15px; font-weight:500; }
  .foresight input,.foresight textarea,.foresight select { background:#fff; color:#101010; border:1px solid var(--rule); padding:11px 13px; border-radius:0; font:17px/1.4 'IBM Plex Sans',sans-serif; width:100%; min-width:0; }
  .foresight textarea { resize:vertical; }.foresight input[type="datetime-local"] { max-width:360px; }
  .foresight button { background:#fff; color:#101010; border:1px solid var(--rule); padding:11px 17px; border-radius:0; font:500 15px/1.4 'IBM Plex Sans',sans-serif; cursor:pointer; }
  .foresight button:hover { background:#f4f1ea; }.foresight button:disabled { opacity:.55; cursor:wait; }
  .foresight button.primary { background:#101010; color:#fff; }.foresight button.primary:hover { background:#333; }
  .foresight :is(a,button,input,select,textarea,summary):focus-visible { outline:3px solid #00aeef; outline-offset:3px; }
  .foresight a { color:#101010; text-decoration:underline; text-underline-offset:3px; overflow-wrap:anywhere; }
  .panel { padding:29px; background:#f4f1ea; margin-top:22px; }.panel h2 { margin-bottom:17px; }
  .small { font-size:15px; color:#6e6960; }.empty { padding-top:29px; padding-bottom:29px; color:#6e6960; }
  .notice,.error { padding:15px 17px; border-left:4px solid #00aeef; background:#ece7dc; overflow-wrap:anywhere; }
  .error { border-color:#ff3b1f; color:#101010; }.notice { color:#101010; }
  .world-intro { padding:29px 0; display:flex; align-items:center; justify-content:space-between; gap:22px; }
  .world-intro h2 { font-size:50px; }.world-actions { display:grid; gap:13px; flex-shrink:0; }
  .record-link { font-size:13px; }.metrics { display:grid; grid-template-columns:repeat(4,1fr); border-top:1px solid var(--rule); border-bottom:1px solid var(--rule); }
  .metrics>div { padding:17px 0; }.metrics span,.metrics strong { display:block; }.metrics strong { font:500 38px/1.3 'Cormorant Garamond',serif; }
  .view-nav { display:flex; gap:8px; padding-top:22px; flex-wrap:wrap; }.view-nav button { border-color:transparent; padding:11px 17px; }.view-nav button[aria-pressed="true"] { border-bottom:3px solid #101010; background:#f6e800; }
  .badge { margin-left:8px; }.event-list { list-style:none; margin:22px 0 0; padding:0; }.event-list>li,.prediction { padding:22px 0; border-top:1px solid var(--rule); }
  .event-title { display:flex; justify-content:space-between; align-items:start; gap:22px; }.event-title h3 { max-width:800px; }.probability { font:500 22px/1.4 'IBM Plex Mono',monospace; flex-shrink:0; }
  .prediction-number { font:500 38px/1.1 'Cormorant Garamond',serif; white-space:nowrap; }.future-grid { display:grid; grid-template-columns:repeat(auto-fit,minmax(min(100%,320px),1fr)); gap:22px; }
  .future { border-top:2px solid var(--rule); padding-top:17px; }.foresight details { margin-top:17px; }.foresight summary { cursor:pointer; font-size:15px; font-weight:500; padding:8px 0; }
  .foresight pre { color:#101010; background:#ece7dc; padding:13px; white-space:pre-wrap; overflow-wrap:anywhere; font:13px/1.55 'IBM Plex Mono',monospace; max-height:320px; overflow:auto; margin:8px 0; }
  .foresight details>a { display:block; margin:8px 0; }.route,.revision { margin:17px 0; padding:0 0 17px 17px; border-left:2px solid var(--rule); }.revision>a { display:block; }
  .future-story { white-space:pre-wrap; overflow-wrap:anywhere; margin:17px 0; font:17px/1.55 'IBM Plex Sans',sans-serif; }
  .required-events { padding-left:22px; }.required-events li { margin:17px 0; }.required-events a { display:block; font-size:15px; }
  .inline-actions { display:flex; flex-wrap:wrap; align-items:center; gap:17px; font-size:15px; }.create-form { display:grid; grid-template-columns:repeat(2,1fr); gap:17px; }.create-form h2,.wide { grid-column:1/-1; }
  .model { display:flex; flex-wrap:wrap; gap:17px; border-top:1px solid var(--rule); border-bottom:1px solid var(--rule); padding:17px 0; margin:22px 0; }
  .comparison { display:grid; grid-template-columns:repeat(3,1fr); gap:17px; margin:22px 0; }.comparison span,.comparison strong { display:block; }.comparison strong { font:500 38px/1.3 'Cormorant Garamond',serif; }
  .change { padding:13px 0; border-top:1px solid var(--rule); }.activity { padding:0; list-style:none; }.activity li { display:grid; grid-template-columns:1fr auto; border-top:1px solid var(--rule); padding:15px 0; }.activity a { grid-column:1/-1; font:13px/1.55 'IBM Plex Mono',monospace; }
  @media(max-width:850px) { .masthead,.world-intro { flex-direction:column; align-items:start; }.foresight h1 { font-size:50px; }.world-actions { display:flex; flex-wrap:wrap; align-items:center; }.metrics { grid-template-columns:repeat(2,1fr); }.panel { padding:22px; } }
  @media(max-width:540px) { .foresight { padding:22px 17px 90px; }.foresight h1,.world-intro h2 { font-size:38px; }.foresight h2 { font-size:29px; }.world-picker,.event-title { align-items:stretch; flex-direction:column; }.world-picker label { width:100%; }.create-form { grid-template-columns:1fr; }.comparison { gap:10px; }.comparison strong { font-size:29px; }.view-nav button { padding:10px; }.panel { padding:17px; }.prediction-number { font-size:29px; } }
</style>
