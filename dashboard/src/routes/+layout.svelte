<script lang="ts">
  import '../app.css';
  import { requiresAgentSetup } from '$lib/setup-routing';
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { base } from '$app/paths';
  import PawLogo from '$lib/components/PawLogo.svelte';
  import ThemeToggle from '$lib/components/ThemeToggle.svelte';
  import { fetchSetupStatus, fetchVersion, checkForUpdates, checkEdgeBuild, triggerRedeploy, getRailwayStatus } from '$lib/api';
  import type { VersionInfo, UpdateCheck, EdgeBuild } from '$lib/api';
  import { getCurrentUser, logout, type SessionUser } from '$lib/auth';
  import { page } from '$app/stores';
  import PawPanel from '$lib/components/PawPanel.svelte';
  import { pawChat, togglePanel } from '$lib/stores/paw-chat';

  let { children } = $props();
  let collapsed = $state(false);
  let user = $state<SessionUser | null>(null);
  let panelOpen = $state(false);

  const unsubPanel = pawChat.subscribe((s) => { panelOpen = s.panelOpen; });
  let authReady = $state(false);
  let authError = $state('');
  let version = $state<VersionInfo | null>(null);
  let updateInfo = $state<UpdateCheck | null>(null);
  let edgeBuild = $state<EdgeBuild | null>(null);
  let updating = $state<'latest' | 'edge' | null>(null);
  let updateError = $state('');
  let updateSuccess = $state('');
  let canUpdate = $state(false);

  function appHref(path: string): string {
    return `${base}${path}`;
  }

  function relativePath(pathname: string): string {
    if (base && pathname.startsWith(base)) {
      return pathname.slice(base.length) || '/';
    }
    return pathname || '/';
  }

  let currentPath = $derived(relativePath($page.url.pathname));
  let isCanvas = $derived(currentPath === '/');
  let isLoginRoute = $derived(currentPath === '/login');
  let isWelcomeRoute = $derived(currentPath === '/welcome');

  onMount(async () => {
    const onLoginPage = relativePath($page.url.pathname) === '/login';

    try {
      user = await getCurrentUser();
      authReady = true;

      if (onLoginPage && user) {
        await goto(appHref('/'));
        return;
      }
      if (!onLoginPage && !user) {
        await goto(appHref('/login'));
        return;
      }

      if (user) {
        // First-run detection: redirect to welcome if setup is incomplete
        const onWelcomePage = relativePath($page.url.pathname) === '/welcome';
        if (!onWelcomePage) {
          try {
            const setupStatus = await fetchSetupStatus();
            if (
              requiresAgentSetup(relativePath($page.url.pathname), setupStatus)
            ) {
              await goto(appHref('/welcome'));
              return;
            }
          } catch {
            // If setup status check fails, proceed normally
          }
        }


        // Fetch version + check for updates in background
        fetchVersion().then(v => { version = v; });
        checkForUpdates().then(u => { updateInfo = u; });
        checkEdgeBuild().then(e => { edgeBuild = e; });
        getRailwayStatus().then(r => { canUpdate = r.can_update; });
      }
    } catch (err) {
      authReady = true;
      authError = err instanceof Error ? err.message : 'Unable to load dashboard';
      if (!onLoginPage) {
        await goto(appHref('/login'));
      }
    }
  });

  onDestroy(unsubPanel);

  async function handleLogout() {
    await logout();
    user = null;
    await goto(appHref('/login'));
  }

  async function handleUpdate(tag: 'latest' | 'edge') {
    updating = tag;
    updateError = '';
    updateSuccess = '';
    try {
      await triggerRedeploy(tag);
      updateSuccess = tag === 'edge'
        ? 'Updating to latest build. This will take a few minutes.'
        : `Updating to ${updateInfo?.latest_version}. This will take a few minutes.`;
    } catch (err) {
      updateError = err instanceof Error ? err.message : 'Update failed';
    } finally {
      updating = null;
    }
  }
</script>

<svelte:window onkeydown={(e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
    e.preventDefault();
    togglePanel();
  }
  if (e.key === 'Escape' && panelOpen) {
    e.preventDefault();
    togglePanel();
  }
}} />

{#if isLoginRoute}
  <main class="auth-main">
    {@render children()}
  </main>
{:else if authReady}
<div class="shell" class:sidebar-collapsed={collapsed}>
  <aside class="sidebar" class:collapsed>
    <div class="sidebar-top">
      <button class="sidebar-toggle" onclick={() => collapsed = !collapsed} title={collapsed ? 'Expand' : 'Collapse'}>
        {#if collapsed}
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M9 18l6-6-6-6"/></svg>
        {:else}
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M15 18l-6-6 6-6"/></svg>
        {/if}
      </button>
      {#if !collapsed}
        <span class="sidebar-title">TEMPER PAW</span>
      {/if}
    </div>

    <nav class="sidebar-nav">
      <a href={appHref('/')} class="nav-item" class:active={currentPath === '/'}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/></svg>
        {#if !collapsed}<span>Canvas</span>{/if}
      </a>
      <a href={appHref('/apps')} class="nav-item" class:active={currentPath === '/apps'}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/></svg>
        {#if !collapsed}<span>Apps</span>{/if}
      </a>
      <a href={appHref('/apps/paw-patrol')} class="nav-item" class:active={currentPath.startsWith('/apps/paw-patrol')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M12 3l7 3v5c0 5-3 8-7 10-4-2-7-5-7-10V6l7-3z"/><path d="M9 12l2 2 4-5"/></svg>
        {#if !collapsed}<span>Patrol</span>{/if}
      </a>
      <a href={appHref('/foresight')} class="nav-item" class:active={currentPath.startsWith('/foresight')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M3 19L9 13L14 16L21 5"/><path d="M15 5H21V11"/></svg>
        {#if !collapsed}<span>Foresight</span>{/if}
      </a>
      <a href={appHref('/sessions')} class="nav-item" class:active={currentPath.startsWith('/sessions')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>
        {#if !collapsed}<span>Sessions</span>{/if}
      </a>
      <button class="nav-item" class:active={panelOpen} onclick={togglePanel}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
        {#if !collapsed}<span>Paw</span>{/if}
      </button>
      <a href={appHref('/settings')} class="nav-item" class:active={currentPath.startsWith('/settings')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06A1.7 1.7 0 0 0 15 19.4a1.7 1.7 0 0 0-1 .6 1.7 1.7 0 0 1-3 0 1.7 1.7 0 0 0-1-.6 1.7 1.7 0 0 0-1.82.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-.6-1 1.7 1.7 0 0 1 0-3 1.7 1.7 0 0 0 .6-1 1.7 1.7 0 0 0-.34-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-.6 1.7 1.7 0 0 1 3 0 1.7 1.7 0 0 0 1 .6 1.7 1.7 0 0 0 1.82-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.7 1.7 0 0 0 19.4 9c.26.3.46.65.6 1a1.7 1.7 0 0 1 0 3c-.14.35-.34.7-.6 1z"/></svg>
        {#if !collapsed}<span>Settings</span>{/if}
      </a>

    </nav>

    <div class="sidebar-footer">
      {#if !collapsed}
        {#if updateSuccess}
          <div class="update-pill update-pill--success">
            <span class="update-pill-text">{updateSuccess}</span>
          </div>
        {/if}
        {#if updateInfo?.update_available}
          <div class="update-pill">
            <a href={updateInfo.release_url || '#'} target="_blank" rel="noopener" class="update-pill-link">
              <span class="update-pill-dot"></span>
              <span class="update-pill-text">{updateInfo.latest_version} available</span>
            </a>
            {#if canUpdate}
              <button class="update-pill-btn" onclick={() => handleUpdate('latest')} disabled={updating !== null}>
                {updating === 'latest' ? '...' : 'Update'}
              </button>
            {/if}
          </div>
        {/if}
        {#if canUpdate || version?.version === 'dev'}
          <button
            class="deploy-btn"
            onclick={() => handleUpdate('edge')}
            disabled={updating !== null || !canUpdate}
          >
            {#if updating === 'edge'}
              Deploying...
            {:else}
              Deploy latest build
            {/if}
          </button>
        {/if}
        {#if updateError}
          <span class="update-error">{updateError}</span>
        {/if}
      {/if}
      {#if user && !collapsed}
        <div class="user-chip">
          <span class="user-email">{user.email}</span>
          <button class="user-logout" type="button" onclick={handleLogout}>Log out</button>
        </div>
      {/if}
      {#if version && !collapsed}
        <span class="version-label">{version.version === 'dev' ? 'dev' : version.version}</span>
      {/if}
      <ThemeToggle />
    </div>
  </aside>

  <main class="main" class:main--canvas={isCanvas} class:main--panel-open={panelOpen}>
    {@render children()}
  </main>

  <!-- Global Paw chat panel -->
  <PawPanel />

  <!-- FAB to open Paw -->
  {#if !panelOpen}
    <button class="paw-fab" onclick={togglePanel} title="Chat with Paw (Cmd+K)" aria-label="Temper Paw chat">
      <PawLogo size={20} />
    </button>
  {/if}
</div>
{:else}
  <main class="auth-main auth-main--loading">
    <p>{authError || 'Checking your session…'}</p>
  </main>
{/if}

<style>
  .auth-main {
    min-height: 100vh;
    min-height: 100dvh;
  }

  .auth-main--loading {
    display: grid;
    place-items: center;
    color: var(--text-2);
  }

  .shell {
    display: flex;
    min-height: 100vh;
    min-height: 100dvh;
  }

  /* ---- Sidebar (desktop) ---- */
  .sidebar {
    width: var(--sidebar-w);
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    padding: var(--sp-3) var(--sp-2);
    border-right: 1px solid var(--border);
    background: var(--bg);
    position: fixed;
    top: 0; left: 0; bottom: 0;
    z-index: 20;
    transition: width var(--duration) var(--ease);
    overflow: hidden;
  }

  .sidebar.collapsed {
    width: 44px;
    padding: var(--sp-3) var(--sp-1);
  }

  .sidebar-top {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding-bottom: var(--sp-4);
    min-height: 20px;
  }

  .sidebar-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px; height: 24px;
    color: var(--text-3);
    flex-shrink: 0;
    border-radius: var(--radius);
  }

  .sidebar-toggle:hover { color: var(--text-1); }

  .sidebar-title {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.08em;
    color: var(--text-2);
    white-space: nowrap;
  }

  /* ---- Nav items ---- */
  .sidebar-nav {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--radius);
    font-size: var(--text-sm);
    color: var(--text-3);
    text-decoration: none;
    transition: color var(--duration) var(--ease), background var(--duration) var(--ease);
    white-space: nowrap;
  }

  .nav-item:hover { color: var(--text-1); background: var(--accent-subtle); text-decoration: none; }
  .nav-item.active { color: var(--text-1); }
  .nav-item svg { flex-shrink: 0; }

  /* ---- Footer ---- */
  .sidebar-footer {
    margin-top: auto;
    padding-top: var(--sp-2);
    display: grid;
    gap: var(--sp-2);
  }

  .user-chip {
    display: grid;
    gap: var(--sp-1);
    padding: var(--sp-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface-1, rgba(255,255,255,0.02));
  }

  .user-email {
    font-size: var(--text-xs);
    color: var(--text-2);
    word-break: break-word;
  }

  .user-logout {
    justify-self: start;
    font-size: var(--text-xs);
    color: var(--text-1);
  }

  .version-label {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--text-3);
    letter-spacing: 0.04em;
    text-align: center;
  }

  /* ---- Sidebar update controls ---- */
  .update-pill {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font-family: var(--font-mono);
    font-size: 11px;
  }

  .update-pill--success {
    border-color: var(--accent, var(--border));
  }

  .update-pill-link {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--text-2);
    text-decoration: none;
    min-width: 0;
    flex: 1;
  }

  .update-pill-link:hover { color: var(--text-1); }

  .update-pill-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent, #4ade80);
    flex-shrink: 0;
  }

  .update-pill-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-2);
    font-size: 11px;
  }

  .update-pill-btn {
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.04em;
    padding: 1px var(--sp-2);
    border: 1px solid var(--text-1);
    border-radius: var(--radius);
    color: var(--bg);
    background: var(--text-1);
    cursor: pointer;
    white-space: nowrap;
    font-weight: 600;
    flex-shrink: 0;
  }

  .update-pill-btn:hover { opacity: 0.85; }
  .update-pill-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .deploy-btn {
    width: 100%;
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.04em;
    padding: var(--sp-1) var(--sp-2);
    border: 1px dashed var(--text-3);
    border-radius: var(--radius);
    color: var(--text-3);
    background: transparent;
    cursor: pointer;
    text-align: center;
  }

  .deploy-btn:hover { color: var(--text-1); border-color: var(--text-2); }
  .deploy-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .update-error {
    color: var(--status-error);
    font-family: var(--font-mono);
    font-size: 10px;
  }

  /* ---- Main content ---- */
  .main {
    flex: 1;
    margin-left: var(--sidebar-w);
    padding: var(--sp-6) var(--sp-8);
    transition: margin-left var(--duration) var(--ease);
  }

  .sidebar-collapsed .main {
    margin-left: 44px;
  }

  .main--canvas {
    padding: 0;
  }

  /* ---- Tablet: collapse sidebar to icons ---- */
  @media (max-width: 768px) {
    .sidebar {
      width: 44px;
      padding: var(--sp-3) var(--sp-1);
    }

    .sidebar-title,
    .nav-item span,
    .user-chip,
    .version-label,
    .update-pill,
    .deploy-btn,
    .update-error { display: none; }

    .main {
      margin-left: 44px;
      padding: var(--sp-4) var(--sp-3);
    }

    .main--canvas { padding: 0; }
  }

  /* ---- Mobile: bottom navigation bar ---- */
  @media (max-width: 640px) {
    .sidebar {
      position: fixed;
      top: auto;
      left: 0;
      right: 0;
      bottom: 0;
      width: 100%;
      height: auto;
      flex-direction: row;
      align-items: center;
      padding: 0;
      border-right: none;
      border-top: 1px solid var(--border);
      z-index: 30;
    }

    .sidebar.collapsed {
      width: 100%;
      padding: 0;
    }

    .sidebar-top,
    .sidebar-footer { display: none; }

    .sidebar-nav {
      flex-direction: row;
      justify-content: space-around;
      align-items: center;
      gap: 0;
      flex: 1;
      padding: var(--sp-2) 0;
      padding-bottom: calc(var(--sp-2) + env(safe-area-inset-bottom, 0px));
    }

    .nav-item {
      flex-direction: column;
      gap: 2px;
      padding: var(--sp-1);
      font-size: 10px;
      min-width: 44px;
      min-height: 44px;
      justify-content: center;
      align-items: center;
      border-radius: var(--radius);
    }

    .nav-item span { display: block; }

    .nav-item svg {
      width: 18px;
      height: 18px;
    }

    .main {
      margin-left: 0;
      margin-bottom: 64px;
      padding: var(--sp-4) var(--sp-4);
    }

    .sidebar-collapsed .main {
      margin-left: 0;
    }

    .main--canvas {
      padding: 0;
      margin-bottom: 64px;
    }

    .paw-fab { display: none; }
  }

  /* ── FAB ── */
  .paw-fab {
    position: fixed;
    bottom: var(--sp-6);
    right: var(--sp-6);
    width: 48px;
    height: 48px;
    border-radius: 50%;
    background: var(--text-1);
    color: var(--bg);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    z-index: 40;
    border: none;
    box-shadow: 0 2px 8px rgba(0,0,0,0.2);
    transition: transform var(--duration) var(--ease), box-shadow var(--duration) var(--ease);
  }

  .paw-fab:hover {
    transform: scale(1.1);
    box-shadow: 0 4px 16px rgba(0,0,0,0.3);
  }

  /* ── Panel open adjustment ── */
  .main--panel-open {
    margin-right: 420px;
    transition: margin-right var(--duration) var(--ease);
  }

  @media (max-width: 640px) {
    .main--panel-open {
      margin-right: 0;
    }
  }
</style>
