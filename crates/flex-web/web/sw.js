// Lets the web UI be installed as an app (Chrome wants a service worker). Nothing is
// cached: everything comes from Flex, which has to be running anyway.
"use strict";
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));
self.addEventListener("fetch", () => {});
