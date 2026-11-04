export interface SessionUser {
  email: string;
  provider: 'local';
}

export interface AuthProviders {
  providers: string[];
  registration_open: boolean;
}

async function authFetch(input: string, init: RequestInit = {}): Promise<Response> {
  return fetch(input, {
    ...init,
    credentials: 'same-origin',
    headers: {
      ...(init.headers as Record<string, string> | undefined),
      'content-type': 'application/json'
    }
  });
}

export async function getAuthProviders(): Promise<AuthProviders> {
  const response = await authFetch('/auth/providers', { method: 'GET', headers: {} });
  if (!response.ok) {
    throw new Error(`Could not load auth providers (${response.status})`);
  }
  return response.json();
}

export async function getCurrentUser(): Promise<SessionUser | null> {
  const response = await authFetch('/auth/me', { method: 'GET', headers: {} });
  if (response.status === 401) {
    return null;
  }
  if (!response.ok) {
    throw new Error(`Could not load current user (${response.status})`);
  }
  return response.json();
}

export async function checkAuth(): Promise<SessionUser> {
  const user = await getCurrentUser();
  if (!user) {
    throw new Error('UNAUTHENTICATED');
  }
  return user;
}

export async function register(email: string, password: string, operatorKey: string): Promise<SessionUser> {
  const response = await authFetch('/auth/register', {
    method: 'POST',
    headers: { authorization: `Bearer ${operatorKey}` },
    body: JSON.stringify({ email, password })
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `Register failed (${response.status})`);
  }
  return response.json();
}

export async function login(email: string, password: string): Promise<SessionUser> {
  const response = await authFetch('/auth/login', {
    method: 'POST',
    body: JSON.stringify({ email, password })
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `Login failed (${response.status})`);
  }
  return response.json();
}

export async function logout(): Promise<void> {
  await authFetch('/auth/logout', {
    method: 'POST',
    headers: {}
  });
}

export async function changePassword(current_password: string, new_password: string): Promise<void> {
  const response = await authFetch('/auth/change-password', {
    method: 'POST',
    body: JSON.stringify({ current_password, new_password })
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `Password change failed (${response.status})`);
  }
}
