export interface Env {
  ASSETS: Fetcher;
  API_BASE_URL: string;
  SUB2API_BASE_URL?: string;
  IFRAME_ALLOW_ORIGINS?: string;
}

function applySecurityHeaders(headers: Headers, env: Env) {
  const extraOrigins = (env.IFRAME_ALLOW_ORIGINS || '')
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean);

  if (extraOrigins.includes('*')) {
    headers.set('Content-Security-Policy', 'frame-ancestors *');
    headers.delete('X-Frame-Options');
  } else {
    const origins = new Set<string>();

    const sub2apiBase = (env.SUB2API_BASE_URL || '').trim();
    if (sub2apiBase) {
      try {
        origins.add(new URL(sub2apiBase).origin);
      } catch {
        // ignore invalid URL
      }
    }

    for (const origin of extraOrigins) {
      origins.add(origin);
    }

    if (origins.size > 0) {
      headers.set('Content-Security-Policy', `frame-ancestors 'self' ${Array.from(origins).join(' ')}`);
      headers.delete('X-Frame-Options');
    } else {
      headers.set('X-Frame-Options', 'SAMEORIGIN');
    }
  }

  headers.set('X-Content-Type-Options', 'nosniff');
  headers.set('Referrer-Policy', 'strict-origin-when-cross-origin');
}

function withSecurityHeaders(response: Response, env: Env): Response {
  const headers = new Headers(response.headers);
  applySecurityHeaders(headers, env);
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

function buildProxyRequest(request: Request, targetBaseUrl: string): Request {
  const incomingUrl = new URL(request.url);
  const targetUrl = new URL(`${incomingUrl.pathname}${incomingUrl.search}`, targetBaseUrl);
  const headers = new Headers(request.headers);

  headers.set('x-forwarded-host', incomingUrl.host);
  headers.set('x-forwarded-proto', incomingUrl.protocol.replace(':', ''));

  const init: RequestInit = {
    method: request.method,
    headers,
    redirect: 'manual',
  };

  if (request.method !== 'GET' && request.method !== 'HEAD') {
    init.body = request.body;
  }

  return new Request(targetUrl.toString(), init);
}

async function proxyToBackend(request: Request, env: Env): Promise<Response> {
  const baseUrl = env.API_BASE_URL?.trim();
  if (!baseUrl) {
    return Response.json({ error: 'API_BASE_URL is not configured' }, { status: 500 });
  }

  return fetch(buildProxyRequest(request, baseUrl));
}

export default {
  async fetch(request, env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === '/healthz' || url.pathname.startsWith('/api/')) {
      return proxyToBackend(request, env);
    }

    const assetResponse = await env.ASSETS.fetch(request);
    return withSecurityHeaders(assetResponse, env);
  },
} satisfies ExportedHandler<Env>;
