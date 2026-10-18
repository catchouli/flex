// The phone remote (#/remote): what's playing on the TV, buttons that press keys in Flex,
// and search across the libraries (and YouTube, when it's on) with "Play on TV".
"use strict";

const Remote = (() => {
  const post = (path, body) =>
    api(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });

  function buzz() {
    try { navigator.vibrate?.(12); } catch (_) {}
  }

  async function press(key) {
    buzz();
    try {
      await post("/api/remote/key", key === "subs-off" ? { action: key } : { key });
    } catch (err) {
      toast(`Couldn't reach Flex: ${err.message}`);
    }
  }

  /** Plays a library file on the TV. */
  async function playOnTv(library, file) {
    buzz();
    try {
      await post("/api/remote/play", { library: Number(library), file: Number(file) });
      toast("Playing on the TV");
    } catch (err) {
      toast(`Couldn't play: ${err.message}`);
    }
  }

  async function playYouTube(v) {
    buzz();
    try {
      await post("/api/remote/play", { youtube: v.id, title: v.title });
      toast("Playing on the TV (YouTube takes a few seconds)");
    } catch (err) {
      toast(`Couldn't play: ${err.message}`);
    }
  }

  const time = (s) => {
    s = Math.max(0, Math.floor(s));
    const hh = Math.floor(s / 3600), mm = Math.floor((s % 3600) / 60), ss = s % 60;
    const two = (n) => String(n).padStart(2, "0");
    return hh ? `${hh}:${two(mm)}:${two(ss)}` : `${mm}:${two(ss)}`;
  };

  function nowPlaying(status) {
    const p = status.playing;
    if (!p) return h("div", { class: "np idle" }, h("div", { class: "dim" }, "Nothing playing on the TV"));
    const fraction = p.duration ? p.position / p.duration : null;
    const times = p.position !== null && p.duration ? `${time(p.position)} / ${time(p.duration)}` : "";
    return h("div", { class: "np" },
      h("div", { class: "t" }, `${p.paused ? "⏸" : "▶"} ${p.title}`),
      times ? h("div", { class: "dim" }, times) : null,
      bar(fraction));
  }

  const button = (label, key, extra = "") =>
    h("button", { class: `rk ${extra}`, "data-key": `r-${key}`, "aria-label": key, onclick: () => press(key) }, label);

  function pad() {
    return h("div", { class: "remote" },
      h("div", { class: "dpad" },
        h("span"), button("▲", "up"), h("span"),
        button("◀", "left"), button("OK", "ok", "ok"), button("▶", "right"),
        h("span"), button("▼", "down"), h("span")),
      h("div", { class: "keys" },
        button("‹ Back", "back"),
        button("⏯", "playpause"),
        button("☰ Options", "menu")),
      h("div", { class: "keys" },
        h("button", { class: "rk small", "data-key": "r-subs", onclick: () => press("subs-off") }, "Subtitles off")));
  }

  function hitRow(hit) {
    const open = hit.show !== null
      ? h("a", { class: "hit", href: `#/show/${hit.library}/${hit.show}`, "data-key": `h${hit.library}-${hit.show}` },
          h("div", { class: "t" }, hit.title), h("div", { class: "d" }, hit.context))
      : h("button", { class: "hit", "data-key": `h${hit.library}-f${hit.file}`, onclick: () => playOnTv(hit.library, hit.file) },
          h("div", { class: "t" }, `▶ ${hit.title}`), h("div", { class: "d" }, hit.context));
    return open;
  }

  function videoRow(v) {
    const mins = v.duration ? ` · ${time(v.duration)}` : "";
    return h("button", { class: "yt", "data-key": `y${v.id}`, onclick: () => playYouTube(v) },
      img(v.thumbnail),
      h("div", {}, h("div", { class: "t" }, v.title), h("div", { class: "d" }, `${v.channel}${mins}`)));
  }

  async function page() {
    let status = await api("/api/remote");
    const np = h("div", {}, nowPlaying(status));
    const local = h("div", { class: "hits" });
    const youtube = h("div", { class: "hits" });
    let query = store("remoteQuery") || "";
    let typed = 0;

    const searchLocal = async (q, mine) => {
      if (!q) return local.replaceChildren();
      try {
        const hits = await api(`/api/search?q=${encodeURIComponent(q)}`);
        if (mine !== typed) return;
        local.replaceChildren(
          h("h2", {}, "In your libraries"),
          hits.length ? hits.map(hitRow) : h("p", { class: "empty" }, "Nothing found."));
      } catch (err) {
        if (mine === typed) local.replaceChildren(h("p", { class: "empty" }, `Search failed: ${err.message}`));
      }
    };
    const searchYouTube = async (q, mine) => {
      if (!q || !status.youtube) return youtube.replaceChildren();
      youtube.replaceChildren(h("h2", {}, "YouTube"), h("p", { class: "empty" }, "Searching YouTube…"));
      try {
        const videos = await api(`/api/youtube/search?q=${encodeURIComponent(q)}`);
        if (mine !== typed) return;
        youtube.replaceChildren(
          h("h2", {}, "YouTube"),
          videos.length ? videos.map(videoRow) : h("p", { class: "empty" }, "Nothing found."));
      } catch (err) {
        if (mine === typed) youtube.replaceChildren(h("h2", {}, "YouTube"), h("p", { class: "empty" }, `Search failed: ${err.message}`));
      }
    };
    let localTimer;
    const changed = (q, now) => {
      query = q.trim();
      store("remoteQuery", query);
      const mine = ++typed;
      clearTimeout(localTimer);
      localTimer = setTimeout(() => searchLocal(query, mine), now ? 0 : 250);
      // YouTube is slow (yt-dlp asks YouTube), so it waits for Enter or a pause in typing.
      if (now) searchYouTube(query, mine);
      else youtube.replaceChildren(query && status.youtube ? h("p", { class: "empty dim" }, "Press Enter to search YouTube too") : "");
    };
    const box = h("input", {
      type: "search", placeholder: status.youtube ? "Search your libraries and YouTube" : "Search your libraries",
      "aria-label": "Search", enterkeyhint: "search", value: query,
      oninput: (e) => changed(e.target.value, false),
      onkeydown: (e) => { if (e.key === "Enter") { e.target.blur(); changed(e.target.value, true); } },
    });
    if (query) changed(query, true);

    // What's playing, every couple of seconds while this page is open.
    const poll = setInterval(async () => {
      if (document.hidden) return;
      try {
        status = await api("/api/remote");
        np.replaceChildren(nowPlaying(status));
      } catch (_) {}
    }, 2000);

    return {
      title: "Remote",
      content: [h("h1", {}, "Remote"), np, pad(), h("div", { class: "tools" }, box), local, youtube],
      destroy: () => { clearInterval(poll); clearTimeout(localTimer); typed++; },
    };
  }

  return { page, playOnTv };
})();
