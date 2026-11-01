<script lang="ts">
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import { fetchOsApps, fetchOsAppGuide, type OsAppEntry } from '$lib/api';

  // Fallback from app.toml manifests (when backend is down)
  const FALLBACK: OsAppEntry[] = [
    { name: 'paw-agent', description: 'Core agent lifecycle — souls, skills, sessions, memory, teams', entity_types: ['Agent', 'Soul', 'Skill', 'Session', 'Memory', 'Team'], version: '0.1.0', app_guide: null },
    { name: 'paw-harness', description: 'Development workflow governance — project harnesses and work cycles', entity_types: ['ProjectHarness', 'WorkCycle'], version: '0.1.0', app_guide: null },
    { name: 'paw-foresight', description: 'Product direction engine — projections, observations, entropy-based simulation', entity_types: ['Direction', 'Projection', 'ProductModel'], version: '0.1.0', app_guide: null },
    { name: 'paw-pm', description: 'Project management — issues, projects, cycles, labels, comments', entity_types: ['Issue', 'Project', 'Cycle', 'Label', 'Comment'], version: '0.1.0', app_guide: null },
    { name: 'paw-channels', description: 'Multi-channel agent communication — Slack, web, API routes', entity_types: ['Channel', 'AgentRoute'], version: '0.1.0', app_guide: null },
    { name: 'paw-compute', description: 'Compute provisioning for agent sandboxes via Fly.io', entity_types: ['Computer'], version: '0.1.0', app_guide: null },
    { name: 'paw-fs', description: 'TemperFS — file, workspace, and directory management', entity_types: ['File', 'Workspace', 'Directory'], version: '0.1.0', app_guide: null },
    { name: 'paw-research', description: 'Governed web search for agent research', entity_types: ['WebQuery'], version: '0.1.0', app_guide: null },
    { name: 'paw-ingest', description: 'Webhook ingress — event routing and processing', entity_types: ['WebhookEvent', 'WebhookRoute'], version: '0.1.0', app_guide: null },
  ];

  let apps = $state<OsAppEntry[]>([]);
  let loaded = $state(false);
  let fromApi = $state(false);
  let expandedApp = $state<string | null>(null);
  let guideCache = $state<Record<string, string>>({});

  onMount(async () => {
    const remote = await fetchOsApps();
    if (remote.length > 0) {
      apps = remote;
      fromApi = true;
    } else {
      apps = FALLBACK;
    }
    loaded = true;
  });

  async function toggleGuide(name: string) {
    if (expandedApp === name) { expandedApp = null; return; }
    expandedApp = name;
    if (!guideCache[name]) {
      // Try API first
      const guide = await fetchOsAppGuide(name);
      if (guide) {
        guideCache = { ...guideCache, [name]: guide };
      } else {
        // Check if the app entry has inline guide
        const app = apps.find(a => a.name === name);
        const inlineGuide = app?.app_guide ?? app?.skill_guide ?? undefined;
        if (inlineGuide) {
          guideCache = { ...guideCache, [name]: inlineGuide };
        }
      }
    }
  }

  function hasGuide(name: string): boolean {
    return !!guideCache[name] || !!apps.find(a => a.name === name)?.skill_guide;
  }
</script>

<div class="page">
  <h1>Apps</h1>
  <p class="desc">{apps.length} OS apps {fromApi ? 'installed' : '(offline — showing catalog)'}</p>

  {#if !loaded}
    <p class="loading">Loading...</p>
  {:else}
    <div class="list">
      {#each apps as app (app.name)}
        <div class="app-row">
          <button class="app-header" onclick={() => toggleGuide(app.name)}>
            <div class="app-main">
              <span class="app-name">{app.name}</span>
              <span class="app-desc">{app.description}</span>
            </div>
            <div class="app-right">
              {#if app.entity_types.length > 0}
                <span class="entity-count">{app.entity_types.length} entities</span>
              {/if}
              <span class="version">v{app.version}</span>
              <span class="chevron">{expandedApp === app.name ? '−' : '+'}</span>
            </div>
          </button>

          {#if expandedApp === app.name}
            <div class="app-detail" transition:slide={{ duration: 150 }}>
              <div class="detail-row">
                <span class="detail-label">Entities</span>
                <div class="entity-list">
                  {#each app.entity_types as et}
                    <span class="entity-tag">{et}</span>
                  {/each}
                  {#if app.entity_types.length === 0}
                    <span class="detail-dim">—</span>
                  {/if}
                </div>
              </div>
              <div class="detail-row">
                <span class="detail-label">Version</span>
                <span class="detail-val">{app.version}</span>
              </div>

              {#if guideCache[app.name]}
                <pre class="guide">{guideCache[app.name]}</pre>
              {:else if app.app_guide}
                <pre class="guide">{app.app_guide}</pre>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .page { max-width: 720px; }
  h1 { margin-bottom: 2px; }
  .desc { font-size: var(--text-sm); color: var(--text-3); margin-bottom: var(--sp-6); }
  .loading { font-family: var(--font-mono); font-size: var(--text-sm); color: var(--text-3); }

  .list { display: flex; flex-direction: column; }
  .app-row { border-bottom: 1px solid var(--border); }

  .app-header {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-3) 0;
    width: 100%;
    text-align: left;
    cursor: pointer;
  }

  .app-header:hover { opacity: 0.8; }

  .app-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .app-name {
    font-family: var(--font-mono);
    font-size: var(--text-base);
    font-weight: 500;
    color: var(--text-1);
  }

  .app-desc {
    font-size: var(--text-sm);
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .app-right {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-shrink: 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
  }

  .entity-count { white-space: nowrap; }
  .version { white-space: nowrap; }

  .chevron {
    font-size: var(--text-sm);
    color: var(--text-3);
    width: 14px;
    text-align: center;
  }

  .app-detail {
    padding: var(--sp-2) 0 var(--sp-3) 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }

  .detail-row {
    display: flex;
    align-items: baseline;
    gap: var(--sp-3);
  }

  .detail-label {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-3);
    min-width: 60px;
    flex-shrink: 0;
  }

  .detail-val {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--text-2);
  }

  .detail-dim {
    color: var(--text-3);
    font-size: var(--text-sm);
  }

  .entity-list {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
    margin-bottom: var(--sp-2);
  }

  .entity-tag {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-2);
    background: var(--surface);
    border: 1px solid var(--border);
    padding: 1px 6px;
    border-radius: var(--radius);
  }

  .guide {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--text-2);
    background: var(--surface);
    border: 1px solid var(--border);
    padding: var(--sp-3);
    border-radius: var(--radius);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 400px;
    overflow-y: auto;
    line-height: 1.5;
  }


  @media (max-width: 500px) {
    .app-right .entity-count,
    .app-right .version { display: none; }
  }
</style>
