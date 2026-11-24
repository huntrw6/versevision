// VerseVision: a minimal service worker. It exists so browsers treat the page as an installable app; it caches nothing,
// because every screen shows live ProPresenter data.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));
self.addEventListener("fetch", (event) => {
  if (event.request.method !== "GET") return;
  event.respondWith(fetch(event.request).catch(() => new Response("VerseVision is not running on this computer.", {status: 503, headers: {"Content-Type": "text/plain"}})));
});
