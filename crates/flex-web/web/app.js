// Flex web UI: Home, a library's shows, a show's episodes, music (music.js) and the phone
// remote (remote.js). Plain
// DOM, no framework, so it stays quick on a slow mini PC. Works by touch, mouse, or arrow keys + Enter
// (a TV remote); Escape/Backspace goes back.
"use strict";

const main = document.getElementById("main");
const backButton = document.getElementById("back");

// ---------------------------------------------------------------------------------------
// Helpers

/** Builds an element: h("div", {class: "x", onclick: f}, child, "text", ...). */
function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === null || v === undefined || v === false) continue;
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

function img(src, alt) {
  if (!src) return null;
  const el = h("img", { src, alt: alt || "", loading: "lazy", decoding: "async" });
  el.addEventListener("error", () => el.remove()); // fall back to the placeholder
  return el;
}

function bar(fraction) {
  if (fraction === null || fraction === undefined) return null;
  return h("div", { class: "bar" }, h("i", { style: `width:${Math.round(fraction * 100)}%` }));
}

function remaining(secs) {
  const mins = Math.max(1, Math.ceil(secs / 60));
  return mins >= 60 ? `${Math.floor(mins / 60)} h ${mins % 60} min left` : `${mins} min left`;
}

function store(key, value) {
  try {
    if (value === undefined) return localStorage.getItem(key);
    localStorage.setItem(key, value);
  } catch (_) {
    return null;
  }
}

let toastTimer;
function toast(msg) {
  const el = document.getElementById("toast");
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), 3500);
}

async function api(path, options) {
  const res = await fetch(path, options);
  if (!res.ok) throw new Error(res.status === 404 ? "Not found" : `Server error (${res.status})`);
  return res.status === 204 ? null : res.json();
}

async function setWatched(library, files, watched) {
  await api("/api/watched", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ library, files, watched }),
  });
}

// ---------------------------------------------------------------------------------------
// Pages

async function homePage() {
  const data = await api("/api/home");
  const page = [];
  if (data.continue.length) {
    page.push(h("h2", {}, "Continue Watching"));
    page.push(h("div", { class: "row" }, data.continue.map(card)));
  }
  page.push(h("h2", {}, "Libraries"));
  page.push(
    h("div", { class: "tiles" },
      data.libraries.map((l) => {
        const music = l.kind === "music";
        const count = music
          ? l.albums === null ? "reading the music…" : `${l.albums.toLocaleString()} albums`
          : `${l.videos.toLocaleString()} videos${l.organised ? "" : " · organising…"}`;
        return h("a", { class: "tile", href: music ? `#/music/${l.id}` : `#/lib/${l.id}`, "data-key": `lib${l.id}` },
          h("b", {}, l.name),
          h("span", { class: "dim" }, count));
      })));
  if (!data.libraries.length) page.push(h("p", { class: "empty" }, "No libraries yet: add them in Flex's Settings."));
  return { title: "Flex", content: page };
}

function card(c) {
  const status = c.next ? "Up next" : c.remaining !== null ? remaining(c.remaining) : "";
  const play = `#/play/${c.library}/${c.file}`;
  const open = (e) => {
    e.preventDefault();
    // Back from the player returns to the show or film's page, like the TV app.
    if (c.show !== null) history.pushState(null, "", `#/show/${c.library}/${c.show}`);
    location.hash = play;
  };
  return h("a", { class: "card", href: play, "data-key": `c${c.file}`, onclick: open },
    img(c.image),
    h("div", { class: "shade" },
      h("div", { class: "t" }, c.title),
      c.episode ? h("div", { class: "e" }, c.episode) : null,
      status ? h("div", { class: "s" }, status) : null),
    bar(c.fraction));
}

// [label, oldest/A first, newest first by default]
const SORTS = {
  title: ["Title", (a, b) => a.title.localeCompare(b.title, undefined, { numeric: true, sensitivity: "base" }), false],
  added: ["Recently added", (a, b) => a.added - b.added, true],
  year: ["Release year", (a, b) => (a.year || 0) - (b.year || 0), true],
};
const direction = (sort, desc) => sort === "title" ? (desc ? "Z to A" : "A to Z") : (desc ? "Newest first" : "Oldest first");
const FILTERS = {
  all: ["All", () => true],
  unwatched: ["Unwatched", (s) => s.unwatched > 0],
  watched: ["Watched", (s) => s.unwatched === 0],
};

async function libraryPage(id) {
  const data = await api(`/api/library/${id}`);
  if (!data.organised) {
    retryTimer = setTimeout(render, 2000);
    return { title: data.name, content: [h("h1", {}, data.name), h("p", { class: "empty" }, "Organising… shows will appear shortly.")] };
  }
  let sort = store(`sort${id}`) || "added";
  if (!SORTS[sort]) sort = "added";
  let desc = store(`desc${id}`) === null ? SORTS[sort][2] : store(`desc${id}`) === "1";
  let filter = store(`filter${id}`) || "all";
  let text = "";
  const grid = h("div", { class: "grid" });
  const setDesc = (v) => { desc = v; store(`desc${id}`, v ? "1" : "0"); fill(); };
  const way = h("button", { class: "way", onclick: () => setDesc(!desc) });
  const fill = () => {
    const words = text.toLowerCase().split(/\s+/).filter(Boolean);
    const shows = data.shows
      .filter(FILTERS[filter][1])
      .filter((s) => words.every((w) => s.title.toLowerCase().includes(w)))
      // Undated last, either way.
      .sort((a, b) => (sort === "year" ? !a.year - !b.year : 0) || (desc ? -1 : 1) * SORTS[sort][1](a, b));
    grid.replaceChildren(...shows.map((s) => poster(data.id, s)));
    way.textContent = desc ? "↓" : "↑";
    way.title = direction(sort, desc);
    way.setAttribute("aria-label", `Sort direction: ${way.title}`);
    if (!shows.length) grid.append(h("p", { class: "empty" }, "Nothing here."));
  };
  const select = (options, value, onchange) =>
    h("select", { onchange: (e) => onchange(e.target.value) },
      Object.entries(options).map(([k, [label]]) => h("option", { value: k, selected: k === value }, label)));
  const tools = h("div", { class: "tools" },
    h("input", { type: "search", placeholder: `Search ${data.name}`, "aria-label": "Search", oninput: (e) => { text = e.target.value; fill(); } }),
    select(SORTS, sort, (v) => { sort = v; store(`sort${id}`, v); setDesc(SORTS[v][2]); }),
    way,
    select(FILTERS, filter, (v) => { filter = v; store(`filter${id}`, v); fill(); }));
  fill();
  return { title: data.name, content: [h("h1", {}, data.name), tools, grid] };
}

function poster(library, s) {
  const badge = s.series
    ? s.unwatched > 0 ? h("span", { class: "badge" }, s.unwatched) : h("span", { class: "badge done" }, "✓")
    : s.unwatched === 0 ? h("span", { class: "badge done" }, "✓") : null;
  const sub = [s.year, s.series ? `${s.episodes} episodes` : null].filter(Boolean).join(" · ");
  return h("a", { class: "poster", href: `#/show/${library}/${s.id}`, "data-key": `s${s.id}` },
    h("div", { class: "art" }, s.title, img(s.poster), badge),
    h("div", { class: "name" }, s.title),
    sub ? h("div", { class: "sub" }, sub) : null);
}

async function showPage(library, id) {
  const data = await api(`/api/show/${library}/${id}`);
  const seasonOf = (file) => data.seasons.find((s) => s.episodes.some((e) => e.file === file));
  const key = `season${library}/${id}`;
  let current = data.seasons.find((s) => String(s.season) === store(key)) || seasonOf(data.next) || data.seasons[0];

  const all = data.seasons.flatMap((s) => s.episodes);
  const allWatched = all.length > 0 && all.every((e) => e.watched);
  const meta = [
    data.year,
    data.genres ? data.genres.split(",").slice(0, 3).join(", ") : null,
    data.runtime ? `${data.runtime} min` : null,
    data.series ? `${all.length} episodes` : null,
  ].filter(Boolean).join(" · ");

  const list = h("div", { class: "episodes" });
  const tabs = h("div", { class: "seasons" });
  const showSeason = (season) => {
    current = season;
    store(key, String(season.season));
    tabs.replaceChildren(...data.seasons.map((s) =>
      h("button", { class: `btn${s === current ? " on" : ""}`, "data-key": `t${s.season}`, onclick: () => showSeason(s) }, s.label)));
    list.replaceChildren(...season.episodes.map((e) => episode(data, e)));
  };
  if (current) showSeason(current);

  const content = [
    h("section", { class: "hero" },
      data.backdrop ? h("div", { class: "bg" }, img(data.backdrop)) : null,
      h("div", { class: "inner" },
        img(data.poster) || h("div", {}),
        h("div", {}, h("h1", {}, data.title), h("div", { class: "meta" }, meta)))),
    data.overview ? h("p", { class: "overview" }, data.overview) : null,
    h("div", { class: "actions" },
      playNext(data, all),
      all.length ? h("button", { class: "btn", "data-key": "all", onclick: () => mark(library, all.map((e) => e.file), !allWatched) },
        allWatched ? "Mark as unwatched" : "Mark as watched") : null),
    data.seasons.length > 1 ? tabs : null,
    list,
  ];
  return { title: data.title, content };
}

function episode(show, e) {
  const label = show.series && e.number !== null ? `${e.number}. ${e.title}` : e.title;
  const sub = e.remaining !== null ? remaining(e.remaining) : e.duration ? `${Math.round(e.duration / 60)} min` : null;
  return h("div", { class: "ep" },
    h("a", { class: "play", href: `#/play/${show.library}/${e.file}`, "data-key": `p${e.file}` },
      h("div", { class: "thumb" }, img(e.image), bar(e.fraction)),
      h("div", {},
        h("div", { class: "t" }, label),
        sub ? h("div", { class: "d" }, sub) : null,
        e.overview ? h("div", { class: "o" }, e.overview) : null)),
    h("button", {
      class: `w${e.watched ? " on" : ""}`, "data-key": `w${e.file}`, title: e.watched ? "Watched" : "Not watched",
      "aria-label": e.watched ? "Mark as unwatched" : "Mark as watched", "aria-pressed": e.watched ? "true" : "false",
      onclick: () => mark(show.library, [e.file], !e.watched),
    }, "✓"),
    h("button", {
      class: "w tv", "data-key": `tv${e.file}`, title: "Play on the TV", "aria-label": "Play on the TV",
      onclick: () => Remote.playOnTv(show.library, e.file),
    }, "📺"));
}

/** "Resume S1E3", "Play S1E4" or "Play": the show's next episode (or the film). */
function playNext(data, all) {
  const next = all.find((e) => e.file === data.next);
  if (!next) return null;
  const season = data.seasons.find((s) => s.episodes.includes(next))?.season;
  const code = data.series && next.number !== null && season ? ` S${season}E${next.number} · ${next.title}` : "";
  const verb = next.remaining !== null ? "Resume" : "Play";
  return [
    h("a", { class: "btn primary", href: `#/play/${data.library}/${next.file}`, "data-key": "play" }, `▶ ${verb}${code}`),
    h("button", { class: "btn", "data-key": "tv", onclick: () => Remote.playOnTv(data.library, next.file) }, "📺 On the TV"),
  ];
}

async function mark(library, files, watched) {
  try {
    await setWatched(library, files, watched);
    render({ keepFocus: true });
  } catch (err) {
    toast(`Couldn't update: ${err.message}`);
  }
}

// ---------------------------------------------------------------------------------------
// Routing

const ROUTES = [
  [/^#?\/?$/, () => homePage()],
  [/^#\/lib\/(\d+)$/, (m) => libraryPage(m[1])],
  [/^#\/show\/(\d+)\/(\d+)$/, (m) => showPage(m[1], m[2])],
  [/^#\/music\/(\d+)$/, (m) => Music.albumsPage(m[1])],
  [/^#\/album\/(\d+)\/(.+)$/, (m) => Music.albumPage(m[1], decodeURIComponent(m[2]))],
  [/^#\/remote$/, () => Remote.page()],
  [/^#\/play\/(\d+)\/(\d+)(?:\?t=([\d.]+))?$/, (m) => Player.page(m[1], m[2], m[3] === undefined ? null : Number(m[3]))],
];

let retryTimer;
/** The page showing (the player has keys and cleanup of its own). */
let currentPage = null;
/** Where each page was left (scroll, focused item), to return to it on Back. */
const positions = new Map();
let usingKeys = false;

async function render(options = {}) {
  clearTimeout(retryTimer);
  const route = location.hash || "#/";
  const saved = options.keepFocus ? { y: scrollY, key: document.activeElement?.dataset?.key } : positions.get(route);
  backButton.hidden = route === "#/";
  // Stop the player (and its stream) before anything else replaces it.
  currentPage?.destroy?.();
  currentPage = null;
  let page;
  const slow = setTimeout(() => main.replaceChildren(h("p", { class: "empty" }, "Loading…")), 400);
  try {
    const [re, handler] = ROUTES.find(([re]) => re.test(route)) || [];
    page = handler ? await handler(route.match(re)) : { title: "Flex", content: [h("p", { class: "empty" }, "Page not found.")] };
  } catch (err) {
    page = { title: "Flex", content: [h("p", { class: "empty" }, `Couldn't load this page: ${err.message}. Is Flex running?`)] };
  }
  clearTimeout(slow);
  if ((location.hash || "#/") !== route) {
    page.destroy?.(); // navigated away meanwhile
    return;
  }
  currentPage = page;
  document.body.classList.toggle("fullscreen", !!page.fullscreen);
  document.title = page.title === "Flex" ? "Flex" : `${page.title} · Flex`;
  main.replaceChildren(...page.content.filter(Boolean));
  if (saved) {
    scrollTo(0, saved.y);
    const el = saved.key && main.querySelector(`[data-key="${CSS.escape(saved.key)}"]`);
    if (el) el.focus({ preventScroll: true });
  } else {
    scrollTo(0, 0);
    if (usingKeys) focusables().find((el) => main.contains(el))?.focus();
  }
}

addEventListener("hashchange", (e) => {
  const old = new URL(e.oldURL).hash || "#/";
  positions.set(old, { y: scrollY, key: document.activeElement?.dataset?.key });
  render();
});
backButton.addEventListener("click", () => history.back());
// Back in Flex after a while: show what's changed (e.g. watched on the TV).
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && !currentPage?.fullscreen) render({ keepFocus: true });
});

// ---------------------------------------------------------------------------------------
// Arrow-key navigation, for a remote or keyboard

function focusables() {
  return [...document.querySelectorAll("a[href], a[tabindex='0'], button, select, input")].filter(
    (el) => !el.disabled && !el.hidden && el.tabIndex >= 0 && el.getClientRects().length > 0,
  );
}

/** The nearest focusable in a direction: closest along it, preferring ones in line. */
function neighbour(from, dir) {
  const a = from.getBoundingClientRect();
  let best = null;
  let bestScore = Infinity;
  for (const el of focusables()) {
    if (el === from) continue;
    const b = el.getBoundingClientRect();
    let along, across;
    if (dir === "ArrowRight") { along = b.left - a.right; across = Math.abs((b.top + b.bottom) / 2 - (a.top + a.bottom) / 2); }
    else if (dir === "ArrowLeft") { along = a.left - b.right; across = Math.abs((b.top + b.bottom) / 2 - (a.top + a.bottom) / 2); }
    else if (dir === "ArrowDown") { along = b.top - a.bottom; across = Math.abs((b.left + b.right) / 2 - (a.left + a.right) / 2); }
    else { along = a.top - b.bottom; across = Math.abs((b.left + b.right) / 2 - (a.left + a.right) / 2); }
    if (along < -8) continue;
    const score = Math.max(along, 0) + across * 2;
    if (score < bestScore) { bestScore = score; best = el; }
  }
  return best;
}

addEventListener("keydown", (e) => {
  if (e.altKey || e.ctrlKey || e.metaKey) return;
  if (currentPage?.onKey?.(e)) {
    e.preventDefault();
    return;
  }
  const el = document.activeElement;
  const typing = el && el.tagName === "INPUT";
  if (e.key.startsWith("Arrow")) {
    usingKeys = true;
    // In the search box, Left/Right move the cursor.
    if (typing && (e.key === "ArrowLeft" || e.key === "ArrowRight")) return;
    e.preventDefault();
    const current = el && el !== document.body ? el : null;
    const next = current ? neighbour(current, e.key) : focusables().find((f) => main.contains(f));
    if (next) {
      next.focus({ preventScroll: true });
      next.scrollIntoView({ block: "nearest", inline: "nearest" });
      // Keep the top bar from covering what's focused.
      if (next.getBoundingClientRect().top < 70) scrollBy(0, next.getBoundingClientRect().top - 90);
    }
  } else if ((e.key === "Escape" || (e.key === "Backspace" && !typing) || e.key === "BrowserBack") && location.hash && location.hash !== "#/") {
    if (typing && e.key === "Escape") return el.blur();
    e.preventDefault();
    history.back();
  }
});
addEventListener("pointerdown", () => (usingKeys = false));

// Installable as an app (Chrome only allows this over HTTPS, or for an address marked
// as secure in chrome://flags; see the README).
if ("serviceWorker" in navigator && window.isSecureContext) {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
}

render();
