export interface AuthSession {
  accessToken?: string;
  authenticatedOffline: boolean;
  disabled: boolean;
}

interface AuthConfig {
  disabled?: boolean;
  issuer?: string;
  client_id?: string;
  resource?: string;
  scopes?: string[];
}

interface Discovery {
  authorization_endpoint: string;
  token_endpoint: string;
  end_session_endpoint?: string;
}

interface Tokens {
  access_token: string;
  refresh_token?: string;
  expires_at: number;
}

const TOKENS = 'xo-oidc-tokens';
const VERIFIER = 'xo-oidc-verifier';
const STATE = 'xo-oidc-state';
const AUTHENTICATED = 'xo-authenticated';

export async function authenticate(): Promise<AuthSession> {
  let config: AuthConfig;
  try {
    const response = await fetch('/.well-known/xo-configuration', { cache: 'no-store' });
    if (!response.ok) throw new Error(`Authentication configuration returned ${response.status}`);
    config = await response.json() as AuthConfig;
  } catch (error) {
    if (localStorage.getItem(AUTHENTICATED) === 'true') {
      return { authenticatedOffline: true, disabled: false };
    }
    throw error;
  }
  if (config.disabled) {
    localStorage.setItem(AUTHENTICATED, 'true');
    return { authenticatedOffline: false, disabled: true };
  }
  if (!config.issuer || !config.client_id || !config.resource) throw new Error('Invalid authentication configuration');
  try {
    const discovery = await discover(config.issuer);
    if (window.location.search.includes('code=')) {
      await finishAuthorization(config, discovery);
      window.history.replaceState({}, '', window.location.pathname);
    }
    let tokens = loadTokens();
    if (tokens && tokens.expires_at <= Date.now() + 60_000 && tokens.refresh_token) {
      tokens = await refresh(config, discovery, tokens.refresh_token).catch(() => undefined);
      if (tokens) saveTokens(tokens);
    }
    if (tokens && tokens.expires_at > Date.now() + 10_000) {
      localStorage.setItem(AUTHENTICATED, 'true');
      return { accessToken: tokens.access_token, authenticatedOffline: false, disabled: false };
    }
    await beginAuthorization(config, discovery);
    return new Promise<AuthSession>(() => undefined);
  } catch (error) {
    if (localStorage.getItem(AUTHENTICATED) === 'true' && !window.location.search.includes('code=')) {
      return { authenticatedOffline: true, disabled: false };
    }
    throw error;
  }
}

export async function logout(wipe: () => Promise<void>) {
  await wipe();
  localStorage.removeItem(TOKENS);
  localStorage.removeItem(AUTHENTICATED);
  sessionStorage.removeItem(VERIFIER);
  sessionStorage.removeItem(STATE);
  window.location.replace('/');
}

async function discover(issuer: string): Promise<Discovery> {
  const response = await fetch(`${issuer.replace(/\/$/, '')}/.well-known/openid-configuration`);
  if (!response.ok) throw new Error(`Pocket ID discovery returned ${response.status}`);
  return response.json() as Promise<Discovery>;
}

async function beginAuthorization(config: AuthConfig, discovery: Discovery) {
  const verifier = randomBase64Url(64);
  const state = randomBase64Url(32);
  sessionStorage.setItem(VERIFIER, verifier);
  sessionStorage.setItem(STATE, state);
  const challenge = base64Url(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier))));
  const url = new URL(discovery.authorization_endpoint);
  url.searchParams.set('client_id', config.client_id!);
  url.searchParams.set('redirect_uri', `${window.location.origin}/`);
  url.searchParams.set('response_type', 'code');
  url.searchParams.set('scope', requestedScope(config));
  url.searchParams.set('resource', config.resource!);
  url.searchParams.set('code_challenge', challenge);
  url.searchParams.set('code_challenge_method', 'S256');
  url.searchParams.set('state', state);
  window.location.replace(url);
}

async function finishAuthorization(config: AuthConfig, discovery: Discovery) {
  const parameters = new URLSearchParams(window.location.search);
  const error = parameters.get('error');
  if (error) throw new Error(parameters.get('error_description') || error);
  const code = parameters.get('code');
  const verifier = sessionStorage.getItem(VERIFIER);
  const expectedState = sessionStorage.getItem(STATE);
  if (!code || !verifier) throw new Error('OIDC callback is missing its code verifier');
  if (!expectedState || parameters.get('state') !== expectedState) throw new Error('OIDC callback state does not match');
  const body = new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    client_id: config.client_id!,
    redirect_uri: `${window.location.origin}/`,
    code_verifier: verifier,
    resource: config.resource!,
  });
  const response = await fetch(discovery.token_endpoint, { method: 'POST', body });
  if (!response.ok) throw new Error(`Pocket ID token exchange returned ${response.status}`);
  const value = await response.json() as { access_token: string; refresh_token?: string; expires_in?: number };
  saveTokens({ access_token: value.access_token, refresh_token: value.refresh_token, expires_at: Date.now() + (value.expires_in ?? 300) * 1000 });
  sessionStorage.removeItem(VERIFIER);
  sessionStorage.removeItem(STATE);
}

async function refresh(config: AuthConfig, discovery: Discovery, refreshToken: string) {
  const body = new URLSearchParams({
    grant_type: 'refresh_token',
    refresh_token: refreshToken,
    client_id: config.client_id!,
    resource: config.resource!,
    scope: requestedScope(config),
  });
  const response = await fetch(discovery.token_endpoint, { method: 'POST', body });
  if (!response.ok) throw new Error('Pocket ID token refresh failed');
  const value = await response.json() as { access_token: string; refresh_token?: string; expires_in?: number };
  return { access_token: value.access_token, refresh_token: value.refresh_token ?? refreshToken, expires_at: Date.now() + (value.expires_in ?? 300) * 1000 };
}

function requestedScope(config: AuthConfig) {
  return ['openid', 'offline_access', ...(config.scopes ?? [])].join(' ');
}

function loadTokens(): Tokens | undefined {
  try {
    const value = localStorage.getItem(TOKENS);
    return value ? JSON.parse(value) as Tokens : undefined;
  } catch {
    return undefined;
  }
}

function saveTokens(tokens: Tokens) {
  localStorage.setItem(TOKENS, JSON.stringify(tokens));
}

function randomBase64Url(bytes: number) {
  const value = new Uint8Array(bytes);
  crypto.getRandomValues(value);
  return base64Url(value);
}

function base64Url(value: Uint8Array) {
  let binary = '';
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
