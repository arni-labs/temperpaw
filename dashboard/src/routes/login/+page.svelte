<script lang="ts">
  import { goto } from '$app/navigation';
  import { base } from '$app/paths';
  import { onMount } from 'svelte';
  import { getAuthProviders, login, register } from '$lib/auth';

  let mode = $state<'login' | 'register'>('login');
  let email = $state('');
  let password = $state('');
  let confirmPassword = $state('');
  let operatorKey = $state('');
  let loading = $state(true);
  let submitting = $state(false);
  let error = $state('');

  onMount(async () => {
    try {
      const providers = await getAuthProviders();
      mode = providers.registration_open ? 'register' : 'login';
    } catch (err) {
      error = err instanceof Error ? err.message : 'Could not load authentication options';
    } finally {
      loading = false;
    }
  });

  async function submit() {
    if (!email.trim()) {
      error = 'Email is required';
      return;
    }
    if (!password.trim()) {
      error = 'Password is required';
      return;
    }
    if (mode === 'register' && !operatorKey.trim()) {
      error = 'Operator key is required';
      return;
    }
    if (mode === 'register' && password !== confirmPassword) {
      error = 'Passwords do not match';
      return;
    }

    submitting = true;
    error = '';

    try {
      if (mode === 'register') {
        await register(email, password, operatorKey.trim());
      } else {
        await login(email, password);
      }
      await goto(`${base}/`);
    } catch (err) {
      error = err instanceof Error ? err.message : 'Authentication failed';
    } finally {
      submitting = false;
    }
  }

  function handleSubmit(event: SubmitEvent) {
    event.preventDefault();
    void submit();
  }
</script>

<svelte:head>
  <title>Temper Paw Login</title>
</svelte:head>

<section class="login-shell">
  <div class="login-card">
    <p class="eyebrow">TEMPER PAW</p>
    <h1>{mode === 'register' ? 'Create your account' : 'Welcome back'}</h1>
    <p class="lede">
      {mode === 'register'
        ? 'This deployment is still unclaimed. Enter its operator key, the TEMPER_API_KEY it runs with, to create the first admin account.'
        : 'Sign in with the admin account for this deployment.'}
    </p>

    {#if loading}
      <p class="status">Loading authentication options…</p>
    {:else}
      <form class="form" onsubmit={handleSubmit}>
        {#if mode === 'register'}
          <label>
            <span>Operator key</span>
            <input bind:value={operatorKey} autocomplete="off" placeholder="TEMPER_API_KEY" type="password" />
          </label>
        {/if}

        <label>
          <span>Email</span>
          <input bind:value={email} autocomplete="email" placeholder="you@example.com" type="email" />
        </label>

        <label>
          <span>Password</span>
          <input bind:value={password} autocomplete={mode === 'register' ? 'new-password' : 'current-password'} placeholder="••••••••" type="password" />
        </label>

        {#if mode === 'register'}
          <label>
            <span>Confirm password</span>
            <input bind:value={confirmPassword} autocomplete="new-password" placeholder="••••••••" type="password" />
          </label>
        {/if}

        {#if error}
          <p class="error">{error}</p>
        {/if}

        <button class="submit" disabled={submitting} type="submit">
          {submitting ? 'Working…' : mode === 'register' ? 'Create account' : 'Sign in'}
        </button>
      </form>
    {/if}
  </div>
</section>

<style>
  .login-shell {
    min-height: 100vh;
    min-height: 100dvh;
    display: grid;
    place-items: center;
    padding: var(--sp-6) var(--sp-4);
    background: var(--bg);
  }

  .login-card {
    width: min(400px, 100%);
    padding: var(--sp-6);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }

  .eyebrow {
    margin: 0 0 var(--sp-2);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    letter-spacing: 0.08em;
    color: var(--text-3);
  }

  h1 {
    margin: 0;
    font-size: var(--text-xl);
  }

  .lede {
    margin: var(--sp-2) 0 var(--sp-6);
    font-size: var(--text-sm);
    color: var(--text-2);
    line-height: 1.5;
  }

  .form {
    display: grid;
    gap: var(--sp-3);
  }

  label {
    display: grid;
    gap: var(--sp-1);
  }

  label span {
    font-size: var(--text-sm);
    color: var(--text-2);
  }

  input {
    width: 100%;
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: rgba(255, 255, 255, 0.04);
    color: var(--text-1);
    font-size: 16px; /* prevents iOS auto-zoom */
  }

  .submit {
    margin-top: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--radius);
    border: none;
    background: var(--accent);
    color: var(--bg);
    font-weight: 600;
    font-size: var(--text-base);
  }

  .submit:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .status,
  .error {
    margin: 0;
    font-size: var(--text-sm);
  }

  .error {
    color: var(--status-error);
  }
</style>
