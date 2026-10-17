// Music in the web UI: a library's albums, an album's tracks, and a player bar along the
// bottom that keeps playing while you browse (and shows on a phone's lock screen).
// Uses h(), img(), store(), api() and toast() from app.js.
"use strict";

const Music = (() => {
  const audio = new Audio();
  audio.preload = "auto";
  /** What's playing: the album (as from /api/album) and the index of its track. */
  let album = null;
  let index = -1;
  let bar = null;

  function time(secs) {
    if (!isFinite(secs)) return "";
    const s = Math.floor(secs);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }

  // -------------------------------------------------------------------------------------
  // Playing

  function play(data, start, shuffle) {
    album = shuffle ? { ...data, tracks: [...data.tracks].sort(() => Math.random() - 0.5) } : data;
    load(shuffle ? 0 : start);
  }

  function load(i) {
    if (!album || i < 0 || i >= album.tracks.length) return stop();
    index = i;
    const track = album.tracks[i];
    audio.src = `/media/${album.library}/${track.file}`;
    audio.play().catch((err) => toast(`Couldn't play: ${err.message}`));
    showBar();
    if ("mediaSession" in navigator) {
      navigator.mediaSession.metadata = new MediaMetadata({
        title: track.title,
        artist: track.artists || album.artist,
        album: album.title,
        artwork: album.cover ? [{ src: album.cover }] : [],
      });
    }
  }

  function stop() {
    audio.pause();
    audio.removeAttribute("src");
    album = null;
    index = -1;
    bar?.remove();
    bar = null;
    document.body.classList.remove("has-music");
  }

  const next = () => load(index + 1);
  const prev = () => (audio.currentTime > 3 ? (audio.currentTime = 0) : load(Math.max(0, index - 1)));
  const toggle = () => (audio.paused ? audio.play() : audio.pause());

  audio.addEventListener("ended", next);
  if ("mediaSession" in navigator) {
    navigator.mediaSession.setActionHandler("play", () => audio.play());
    navigator.mediaSession.setActionHandler("pause", () => audio.pause());
    navigator.mediaSession.setActionHandler("nexttrack", next);
    navigator.mediaSession.setActionHandler("previoustrack", prev);
  }
  // A video takes over: pause the music.
  addEventListener("hashchange", () => {
    if (location.hash.startsWith("#/play/")) audio.pause();
  });

  function showBar() {
    const track = album.tracks[index];
    const seek = h("input", {
      type: "range", min: 0, max: 1000, value: 0, "aria-label": "Position", class: "seek",
      oninput: (e) => { if (audio.duration) audio.currentTime = (e.target.value / 1000) * audio.duration; },
    });
    const clock = h("span", { class: "clock" });
    const playButton = h("button", { class: "btn", "aria-label": "Play or pause", onclick: toggle }, "⏸");
    const el = h("div", { class: "music-bar" },
      h("a", { class: "now", href: `#/album/${album.library}/${encodeURIComponent(album.key)}` },
        img(album.cover),
        h("div", {}, h("div", { class: "t" }, track.title), h("div", { class: "d" }, track.artists || album.artist))),
      h("div", { class: "controls" },
        h("button", { class: "btn", "aria-label": "Previous", onclick: prev }, "⏮"),
        playButton,
        h("button", { class: "btn", "aria-label": "Next", onclick: next }, "⏭"),
        h("button", { class: "btn", "aria-label": "Stop", onclick: stop }, "✕")),
      h("div", { class: "progress" }, seek, clock));
    const update = () => {
      playButton.textContent = audio.paused ? "▶" : "⏸";
      if (audio.duration) seek.value = Math.round((audio.currentTime / audio.duration) * 1000);
      clock.textContent = `${time(audio.currentTime)} / ${time(audio.duration)}`;
    };
    for (const ev of ["timeupdate", "play", "pause", "loadedmetadata"]) audio.addEventListener(ev, update);
    bar?.remove();
    bar = el;
    document.body.append(el);
    document.body.classList.add("has-music");
    update();
  }

  // -------------------------------------------------------------------------------------
  // Pages

  const SORTS = {
    added: ["Recently added", (a, b) => b.added - a.added],
    title: ["A–Z", (a, b) => a.title.localeCompare(b.title, undefined, { numeric: true, sensitivity: "base" })],
    artist: ["Artist", (a, b) => a.artist.localeCompare(b.artist, undefined, { sensitivity: "base" }) || (a.year || 0) - (b.year || 0)],
    year: ["Release year", (a, b) => (b.year || 0) - (a.year || 0)],
  };

  async function albumsPage(id) {
    const data = await api(`/api/music/${id}`);
    if (data.loading) {
      retryTimer = setTimeout(render, 3000);
      return { title: data.name, content: [h("h1", {}, data.name), h("p", { class: "empty" }, "Reading the music… albums will appear shortly.")] };
    }
    let sort = store(`sort${id}`) || "added";
    let text = "";
    const grid = h("div", { class: "grid albums" });
    const fill = () => {
      const words = text.toLowerCase().split(/\s+/).filter(Boolean);
      const albums = data.albums
        .filter((a) => words.every((w) => `${a.title} ${a.artist}`.toLowerCase().includes(w)))
        .sort(SORTS[sort][1]);
      grid.replaceChildren(...albums.map((a) => tile(id, a)));
      if (!albums.length) grid.append(h("p", { class: "empty" }, "Nothing here."));
    };
    const tools = h("div", { class: "tools" },
      h("input", { type: "search", placeholder: `Search ${data.name}`, "aria-label": "Search", oninput: (e) => { text = e.target.value; fill(); } }),
      h("select", { onchange: (e) => { sort = e.target.value; store(`sort${id}`, sort); fill(); } },
        Object.entries(SORTS).map(([k, [label]]) => h("option", { value: k, selected: k === sort }, label))));
    fill();
    return { title: data.name, content: [h("h1", {}, data.name), tools, grid] };
  }

  function tile(library, a) {
    return h("a", { class: "poster", href: `#/album/${library}/${encodeURIComponent(a.key)}`, "data-key": `a${a.key}` },
      h("div", { class: "art square" }, a.title, img(a.cover)),
      h("div", { class: "name" }, a.title),
      h("div", { class: "sub" }, [a.artist, a.year].filter(Boolean).join(" · ")));
  }

  async function albumPage(library, key) {
    const data = await api(`/api/album/${library}?key=${encodeURIComponent(key)}`);
    const discs = new Set(data.tracks.map((t) => t.disc).filter(Boolean)).size > 1;
    const meta = [data.artist, data.year, `${data.tracks.length} tracks`, `${Math.round(data.duration / 60)} min`].filter(Boolean).join(" · ");
    const rows = data.tracks.map((t, i) => {
      const number = t.number !== null ? (discs && t.disc ? `${t.disc}.${t.number}` : `${t.number}`) : `${i + 1}`;
      return h("button", { class: "track", "data-key": `t${t.file}`, onclick: () => play(data, i, false) },
        h("span", { class: "n" }, number),
        h("span", { class: "t" }, t.title, t.artists ? h("span", { class: "dim" }, ` · ${t.artists}`) : null),
        h("span", { class: "d" }, time(t.duration)));
    });
    const content = [
      h("section", { class: "hero album" },
        h("div", { class: "inner" },
          img(data.cover) || h("div", {}),
          h("div", {}, h("h1", {}, data.title), h("div", { class: "meta" }, meta)))),
      h("div", { class: "actions" },
        h("button", { class: "btn primary", "data-key": "play", onclick: () => play(data, 0, false) }, "▶ Play"),
        h("button", { class: "btn", "data-key": "shuffle", onclick: () => play(data, 0, true) }, "Shuffle")),
      h("div", { class: "tracks" }, rows),
    ];
    return { title: data.title, content };
  }

  return { albumsPage, albumPage };
})();
