// The phone remote (#/remote): what's playing on the TV, buttons that press keys in Flex,
// and search with "Play on TV", in one scope at a time like the TV's search (YouTube is
// only searched when it's the scope picked).
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
      // Keys are pressed in Flex; actions ("subs-off", "volume-up"…) are carried out.
      await post("/api/remote/key", key.includes("-") ? { action: key } : { key });
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

  /** A volume button: one step per tap, repeating while held. */
  function volume(label, action) {
    let timer = null;
    const stop = () => { clearInterval(timer); timer = null; };
    const start = (e) => {
      e.preventDefault(); // no long-press menu or text selection on phones
      stop();
      press(action);
      timer = setInterval(() => press(action), 250);
    };
    return h("button", {
      class: "rk", "data-key": `r-${action}`, "aria-label": action === "volume-up" ? "Volume up" : "Volume down",
      onpointerdown: start, onpointerup: stop, onpointerleave: stop, onpointercancel: stop, oncontextmenu: (e) => e.preventDefault(),
      // Keyboard (Enter/Space) clicks have no pointer events.
      onclick: (e) => { if (e.detail === 0) press(action); },
    }, label);
  }

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
        volume("🔉 −", "volume-down"),
        volume("🔊 +", "volume-up")),
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

  /** The scopes along the top of the results, like the TV's search. */
  function scopes(libraries, youtube) {
    return [
      ["titles", "Movies & shows"],
      ...libraries.filter((l) => l.kind !== "music").map((l) => [`library:${l.id}`, l.name]),
      ["episodes", "Episodes"],
      ...(youtube ? [["youtube", "YouTube"]] : []),
    ];
  }

  async function page() {
    let [status, home] = await Promise.all([api("/api/remote"), api("/api/home")]);
    const np = h("div", {}, nowPlaying(status));
    const results = h("div", { class: "hits" });
    const tabs = h("div", { class: "seasons scopes" });
    const all = scopes(home.libraries, status.youtube);
    let scope = all.some(([k]) => k === store("remoteScope")) ? store("remoteScope") : "titles";
    let query = store("remoteQuery") || "";
    let typed = 0;
    let timer;

    const run = async (mine) => {
      if (!query) return results.replaceChildren();
      const youtube = scope === "youtube";
      if (youtube) results.replaceChildren(h("p", { class: "empty" }, "Searching YouTube…"));
      try {
        const found = youtube
          ? await api(`/api/youtube/search?q=${encodeURIComponent(query)}`)
          : await api(`/api/search?q=${encodeURIComponent(query)}&scope=${encodeURIComponent(scope)}`);
        if (mine !== typed) return;
        results.replaceChildren(...(found.length ? found.map(youtube ? videoRow : hitRow) : [h("p", { class: "empty" }, "Nothing found.")]));
      } catch (err) {
        if (mine === typed) results.replaceChildren(h("p", { class: "empty" }, `Search failed: ${err.message}`));
      }
    };
    /** Searches the current scope: straight away, or once typing pauses (longer for
     * YouTube, which is slow because yt-dlp has to ask YouTube). */
    const search = (now) => {
      const mine = ++typed;
      clearTimeout(timer);
      timer = setTimeout(() => run(mine), now ? 0 : scope === "youtube" ? 900 : 250);
    };
    const showTabs = () =>
      tabs.replaceChildren(...all.map(([key, label]) =>
        h("button", {
          class: `btn${key === scope ? " on" : ""}`, "data-key": `sc-${key}`,
          onclick: () => { scope = key; store("remoteScope", key); showTabs(); search(true); },
        }, label)));
    showTabs();

    const box = h("input", {
      type: "search", placeholder: "Search", "aria-label": "Search", enterkeyhint: "search", value: query,
      oninput: (e) => { query = e.target.value.trim(); store("remoteQuery", query); search(false); },
      onkeydown: (e) => { if (e.key === "Enter") { e.target.blur(); search(true); } },
    });
    if (query) search(true);

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
      content: [h("h1", {}, "Remote"), np, pad(), h("div", { class: "tools" }, box), tabs, results],
      destroy: () => { clearInterval(poll); clearTimeout(timer); typed++; },
    };
  }

  return { page, playOnTv };
})();
