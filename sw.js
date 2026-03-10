var cacheName = 'egui-template-pwa';
var filesToCache = [
  './',
  './index.html',
  './befunge_editor.js',
  './befunge_editor_bg.wasm',
];
var fetchArgs = { cache: 'no-store', headers: { "x-magic-no-cache": '1' } };

async function isOutdated() {
  let resp;
  try {
    resp = await fetch("./version.json", fetchArgs);
  } catch (e) {
    console.error(e);
    return false;
  }
  if (!resp.ok) return false;

  const { version } = await resp.json();
  const localVersion = await getStoredVersion();

  if (version !== localVersion) {
    console.log(`outdated: ${localVersion}, new: ${version}`)
    await storeVersion(version);
    return true;
  }

  return false;
}

async function refreshCache(cache) {
  await Promise.all(
    filesToCache.map(async (url) => {
      try {
        const resp = await fetch(url, fetchArgs);
        if (resp.ok) {
          await cache.put(url, resp);
        }
      } catch (err) {
        console.warn(err);
      }
    })
  );
}

/* Start the service worker and cache all of the app's content */
self.addEventListener('install', (e) => {
  e.waitUntil(
    caches.open(cacheName).then((cache) => {
      return cache.addAll(filesToCache);
    })
  )
});

/* Serve cached content when offline */
self.addEventListener('fetch', async (e) => {
  if (e.request.headers.get("x-magic-no-cache")) return;
  e.respondWith(serveCached(e.request));
});

async function serveCached(request) {
  if (await isOutdated()) {
    caches.open(cacheName).then(async (cache) => {
      await refreshCache(cache);
    })
  };

  const cached = await caches.match(request)
  return cached ?? fetch(request, fetchArgs);
}

async function getStoredVersion() {
  const cache = await caches.open('sw-meta');
  const resp = await cache.match('version');
  if (!resp) return null;
  return resp.text();
}

async function storeVersion(version) {
  const cache = await caches.open('sw-meta');
  await cache.put('version', new Response(version));
}
