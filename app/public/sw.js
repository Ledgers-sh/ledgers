/*
 * Service worker: caches the app shell and read views (GET) only.
 * Writes (POST, reversals, bill posts) are never queued or replayed; when the
 * server is unreachable the request fails and the UI says "offline: posting needs the server".
 * The one exception is a file shared into the app (POST /share-target): it is
 * stored in the upload queue and uploaded by the app once the device is online.
 *
 * API responses are cached per credential: the cache name carries a SHA-256 of the
 * Authorization header, so one key never reads another key's cached data.
 * Signing out deletes every "ledgers-api-*" cache (see src/app.tsx).
 */
const SHELL_CACHE = 'ledgers-shell-v1';
const SHELL_URLS = ['/', '/index.html', '/manifest.webmanifest', '/favicon.svg', '/assets/apple-touch-icon.png', '/assets/icon-512.png'];
const API_PATH = /\/v1\/(reports|entries|accounts)(\/|$|\?)/;

self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(SHELL_CACHE).then((cache) => cache.addAll(SHELL_URLS)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    (async () => {
      for (const name of await caches.keys()) {
        if (name.startsWith('ledgers-shell-') && name !== SHELL_CACHE) await caches.delete(name);
      }
      await self.clients.claim();
    })(),
  );
});

async function digest(text) {
  const bytes = new TextEncoder().encode(text);
  const hash = await crypto.subtle.digest('SHA-256', bytes);
  return [...new Uint8Array(hash)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

async function apiCacheName(request) {
  const auth = request.headers.get('Authorization') || '';
  return `ledgers-api-${(await digest(auth)).slice(0, 32)}`;
}

async function networkFirst(request, cacheName) {
  const cache = await caches.open(cacheName);
  try {
    const response = await fetch(request);
    if (response.ok) cache.put(request, response.clone());
    return response;
  } catch (error) {
    const cached = await cache.match(request);
    if (cached) return cached;
    throw error;
  }
}

self.addEventListener('push', (event) => {
  const payload = event.data.json();
  const { title, body, url, data = {} } = payload;
  const notificationOptions = {
    body: body || 'New approval request',
    icon: '/assets/icon-512.png',
    badge: '/favicon.svg',
    tag: data.approval_id || 'approval',
    requireInteraction: data.high_risk || false,
    data: {
      approval_id: data.approval_id,
      high_risk: data.high_risk,
      url: url || '/approvals',
      actions: data.actions || [],
    },
    actions: (data.actions || []).slice(0, 2), // Max 2 actions
  };
  event.waitUntil(self.registration.showNotification(title || 'Ledgers', notificationOptions));
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const { url, actions = [] } = event.notification.data;
  // Find the action URL from the action ID
  const action = actions.find((a) => a.action === event.action);
  const targetUrl = action ? action.url : url;
  event.waitUntil(clients.openWindow(targetUrl || '/approvals'));
});

/*
 * Web Share Target (issue #34): a PDF or photo shared from another app arrives
 * here as a multipart POST. The files go into the upload queue in IndexedDB
 * (the same database and schema as src/lib/uploadQueue.ts), then the app opens
 * the Bills screen, which uploads them once the device is online.
 */
const QUEUE_DB = 'ledgers-uploads';
const QUEUE_STORE = 'uploads';

function openQueue() {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(QUEUE_DB, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains(QUEUE_STORE)) {
        open.result.createObjectStore(QUEUE_STORE, { keyPath: 'sha256' });
      }
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
}

async function queueSharedFiles(request) {
  const form = await request.formData();
  const files = form.getAll('files').filter((file) => file instanceof File);
  const db = await openQueue();
  let stored = 0;
  for (const file of files) {
    const bytes = await file.arrayBuffer();
    const digest = await crypto.subtle.digest('SHA-256', bytes);
    const sha256 = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
    const entry = { sha256, name: file.name, type: file.type, blob: new Blob([bytes], { type: file.type }), queuedAt: new Date().toISOString() };
    await new Promise((resolve, reject) => {
      const tx = db.transaction(QUEUE_STORE, 'readwrite');
      tx.objectStore(QUEUE_STORE).put(entry);
      tx.oncomplete = resolve;
      tx.onerror = () => reject(tx.error);
    });
    stored += 1;
  }
  db.close();
  return stored;
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  const shareUrl = new URL(request.url);
  if (request.method === 'POST' && shareUrl.origin === self.location.origin && shareUrl.pathname === '/share-target') {
    event.respondWith(
      queueSharedFiles(request)
        .then(() => Response.redirect('/#/bills?shared=1', 303))
        .catch(() => Response.redirect('/#/bills?shared=failed', 303)),
    );
    return;
  }
  if (request.method !== 'GET') return; // writes always go to the network

  const url = new URL(request.url);
  if (url.origin === self.location.origin && !url.pathname.startsWith('/v1/')) {
    if (request.mode === 'navigate') {
      event.respondWith(
        fetch(request).catch(() => caches.match('/index.html').then((r) => r || caches.match('/'))),
      );
      return;
    }
    // Hashed build assets and static files: cache first, then network.
    event.respondWith(
      caches.match(request).then((cached) => {
        if (cached) return cached;
        return fetch(request).then((response) => {
          if (response.ok && response.type === 'basic') {
            caches.open(SHELL_CACHE).then((cache) => cache.put(request, response.clone()));
          }
          return response;
        });
      }),
    );
    return;
  }

  if (API_PATH.test(url.pathname)) {
    event.respondWith(
      (async () => {
        const name = await apiCacheName(request);
        return networkFirst(request, name);
      })(),
    );
  }
});
