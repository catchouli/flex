// Text subtitles in the web player, drawn by JASSUB (libass compiled to WebAssembly, so
// anime typesetting and the file's own fonts look as they do in mpv).
//
// Extracting a track takes a while over a network share, so lines are shown as they
// arrive: Flex sends the whole track (cached, or the full extraction as it goes) and, when
// playback is somewhere it hasn't reached yet, a quick extraction from there. Lines from
// both are merged.
"use strict";

const Subtitles = (() => {
  let jassub = null; // the JASSUB module, loaded the first time subtitles are shown
  const CJK = /[぀-ヿ㐀-鿿가-힯＀-￯]/;
  const DIALOGUE_START = /^Dialogue:\s*[^,]*,(\d+):(\d+):(\d+(?:\.\d+)?),/;

  function startTime(line) {
    const m = DIALOGUE_START.exec(line);
    return m ? Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3]) : null;
  }

  class Track {
    /**
     * @param opts {video, library, file, index, fonts: [url], cjkFont: family or null,
     *   timeOffset: seconds the video's time is behind the file's}
     */
    constructor(opts) {
      this.opts = opts;
      this.header = null;
      this.seen = new Set();
      this.pending = []; // lines not yet given to the renderer
      this.fetches = []; // {from, done, last, controller}
      this.renderer = null;
      this.creating = null;
      this.cjk = false;
      this.flushTimer = null;
      this.destroyed = false;
      this.timeOffset = opts.timeOffset || 0;
    }

    /** Makes sure lines around `at` (seconds into the file) are coming. */
    need(at) {
      const covered = this.fetches.some((f) => f.from <= at + 1 && (f.done || f.last >= at || at - f.last < 60));
      if (covered) return;
      // Only one quick extraction at a time: a newer seek replaces it.
      for (const f of this.fetches) if (!f.done && f.from > 0) f.controller.abort();
      this.fetches = this.fetches.filter((f) => f.done || f.from === 0);
      this.fetch(at < 1 ? 0 : at);
    }

    async fetch(from) {
      const { library, file, index } = this.opts;
      const f = { from, done: false, last: from, controller: new AbortController() };
      this.fetches.push(f);
      try {
        const res = await fetch(`/subs/${library}/${file}/${index}?from=${from.toFixed(1)}`, { signal: f.controller.signal });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let rest = "";
        let header = [];
        let inEvents = false;
        for (;;) {
          const { value, done } = await reader.read();
          const text = rest + (done ? decoder.decode() : decoder.decode(value, { stream: true }));
          const lines = text.split(/\r?\n/);
          rest = done ? "" : lines.pop();
          for (const line of lines) {
            if (!inEvents) {
              header.push(line);
              if (/^Format:/.test(line) && header.some((l) => /^\[Events\]/i.test(l))) {
                inEvents = true;
                if (this.header === null) this.header = header.join("\n");
              }
              continue;
            }
            if (!line.startsWith("Dialogue:")) continue;
            const t = startTime(line);
            if (t !== null && t > f.last) f.last = t;
            if (this.seen.has(line)) continue;
            this.seen.add(line);
            this.pending.push(line);
            if (!this.cjk && CJK.test(line)) this.useCjk();
          }
          this.schedule();
          if (done) break;
        }
        f.done = true;
      } catch (err) {
        if (err.name !== "AbortError") console.warn("subtitles:", err);
        this.fetches = this.fetches.filter((x) => x !== f);
      }
    }

    /** Hands new lines to the renderer soon (batched: a whole track arrives in pieces). */
    schedule() {
      if (this.flushTimer || this.destroyed || this.header === null || !this.pending.length) return;
      this.flushTimer = setTimeout(() => {
        this.flushTimer = null;
        this.flush();
      }, this.renderer ? 500 : 0);
    }

    async flush() {
      if (this.destroyed || this.header === null || !this.pending.length) return;
      if (!this.renderer) {
        // Lines arriving meanwhile wait in `pending` for the new renderer.
        this.creating ??= this.create();
        return;
      }
      const lines = this.pending.splice(0);
      await this.renderer.ready;
      this.renderer.renderer.processData(lines.join("\n"));
    }

    async create() {
      jassub ??= import("/vendor/jassub/jassub.js");
      const JASSUB = (await jassub).default;
      if (this.destroyed) return;
      const firstLines = this.pending.splice(0);
      const { video, fonts, cjkFont } = this.opts;
      const available = { "liberation sans": "/vendor/jassub/default.woff2" };
      if (cjkFont) available[cjkFont.toLowerCase()] = "/fonts/fallback";
      this.renderer = new JASSUB({
        video,
        subContent: `${this.header}\n${firstLines.join("\n")}\n`,
        workerUrl: "/vendor/jassub/worker.js",
        wasmUrl: "/vendor/jassub/jassub-worker.wasm",
        modernWasmUrl: "/vendor/jassub/jassub-worker-modern.wasm",
        fonts,
        availableFonts: available,
        defaultFont: this.cjk && cjkFont ? cjkFont : "liberation sans",
        timeOffset: this.timeOffset,
      });
      await this.renderer.ready;
      this.schedule(); // lines that arrived meanwhile
    }

    /** CJK text: make the CJK font the fallback (its Latin letters are fine too). */
    useCjk() {
      this.cjk = true;
      const family = this.opts.cjkFont;
      if (!family || !this.renderer) return; // create() picks it up
      this.renderer.ready.then(() => this.renderer.renderer.setDefaultFont(family.toLowerCase()));
    }

    /** The file's time = the video's time + offset (streams start partway through). */
    setTimeOffset(offset) {
      this.timeOffset = offset;
      if (this.renderer) this.renderer.timeOffset = offset;
    }

    destroy() {
      this.destroyed = true;
      clearTimeout(this.flushTimer);
      for (const f of this.fetches) f.controller.abort();
      this.renderer?.destroy();
      this.renderer = null;
    }
  }

  return { Track };
})();
