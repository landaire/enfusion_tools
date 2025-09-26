var cacheName = "enfusion_tools-pwa";
var filesToCache = [
  "./", 
  "./index.html",
  "./ui.js",
  "./ui_bg.wasm"
];

/* Start the service worker and cache all of the app's content */
self.addEventListener("install", function (e) {
  e.waitUntil(
    caches.open(cacheName).then(function (cache) {
      return cache.addAll(filesToCache);
    }),
  );
});

/* Serve cached content when offline */
self.addEventListener("fetch", function (e) {
  e.respondWith(
    caches.match(e.request).then(function (response) {
      // If there's an active network connection, always return the latest
      if (navigator.onLine) {
        return fetch(e.request);
      }
      return response || fetch(e.request);
    }),
  );
});
