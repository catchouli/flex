// The web player: plays a file as is when the browser can, otherwise as an MP4 stream
// that Flex makes with ffmpeg (copying what the browser can decode, converting the
// rest). Keys, like the TV app: Enter/Space pause, Left/Right seek 10s, Up switches
// audio, Down switches subtitles, Escape/Backspace goes back. Text subtitles are drawn
// by subtitles.js; picture subtitles (Blu-ray, DVD) are burned in by ffmpeg. Uses h(),
// api(), toast() and store() from app.js.
"use strict";

const Player = (() => {
  const SEEK_STEP = 10;
  const SAVE_EVERY_MS = 15000;
  const HIDE_CONTROLS_MS = 3000;

  // ---- What the browser can decode ------------------------------------------------------

  const tester = document.createElement("video");
  function supports(mime) {
    try {
      if (window.MediaSource && MediaSource.isTypeSupported(mime)) return true;
    } catch (_) {}
    return tester.canPlayType(mime) !== "";
  }

  function videoCodec(v) {
    switch (v.codec) {
      case "h264": return v.bits > 8 ? null : "avc1.640029"; // no browser decodes 10-bit H.264
      case "hevc": return v.bits > 8 ? "hvc1.2.4.L153.B0" : "hvc1.1.6.L153.B0";
      case "av1": return v.bits > 8 ? "av01.0.09M.10" : "av01.0.09M.08";
      case "vp9": return v.bits > 8 ? "vp09.02.41.10" : "vp09.00.41.08";
      default: return null;
    }
  }
  const AUDIO_CODECS = { aac: "mp4a.40.2", mp3: "mp4a.69", opus: "opus", flac: "flac", ac3: "ac-3", eac3: "ec-3" };

  /**
   * How to play: {direct} for the file itself, else what the stream does to video/audio
   * (and which picture subtitles it burns in, which means converting the video).
   */
  function plan(media, audioIndex, forceConvert, subtitle) {
    const burn = subtitle !== null && media.subtitles[subtitle]?.image ? subtitle : null;
    const v = media.video;
    const a = media.audio[audioIndex];
    const vc = v && videoCodec(v);
    const videoOk = !forceConvert && (!v || (vc && supports(`video/mp4; codecs="${vc}"`)));
    const ac = a && AUDIO_CODECS[a.codec];
    const audioOk = !forceConvert && (!a || (ac && supports(`audio/mp4; codecs="${ac}"`)));
    const mp4 = /(^|,)(mp4|mov)(,|$)/.test(media.container);
    return {
      direct: mp4 && videoOk && audioOk && audioIndex === 0 && burn === null,
      video: videoOk && burn === null ? "copy" : "h264",
      audio: audioOk ? "copy" : "aac",
      burn,
    };
  }

  // ---- Helpers --------------------------------------------------------------------------

  function clock(secs) {
    secs = Math.max(0, Math.floor(secs || 0));
    const h = Math.floor(secs / 3600), m = Math.floor(secs / 60) % 60, s = secs % 60;
    const mm = h ? String(m).padStart(2, "0") : String(m);
    return `${h ? h + ":" : ""}${mm}:${String(s).padStart(2, "0")}`;
  }

  function subtitleLabel(s, i) {
    const lang = s.language ? s.language.toUpperCase() : null;
    const parts = [s.title, lang, s.forced && !/forced/i.test(s.title || "") ? "forced" : null].filter(Boolean);
    return `${parts.length ? parts.join(" · ") : `Subtitles ${i + 1}`}${s.image ? " (burned in)" : ""}`;
  }

  /** The track to show at first: a forced or default one, as mpv would. */
  function defaultSubtitle(media) {
    const t = media.subtitles.find((s) => s.forced) || media.subtitles.find((s) => s.default);
    return t ? t.index : null;
  }

  function trackLabel(a, i) {
    const lang = a.language ? a.language.toUpperCase() : null;
    const parts = [a.title, lang, a.codec.toUpperCase(), a.channels > 2 ? `${a.channels}ch` : null].filter(Boolean);
    return parts.length ? parts.join(" · ") : `Track ${i + 1}`;
  }

  // ---- The page -------------------------------------------------------------------------

  async function page(library, file, startAt) {
    const info = await api(`/api/play/${library}/${file}`);
    const title = info.episode ? `${info.title} · ${info.episode}` : info.title;
    if (info.error || !info.media) {
      return {
        title,
        content: [h("h1", {}, title), h("p", { class: "empty" }, info.error || "This file can't be played here.")],
      };
    }
    const media = info.media;
    const duration = info.duration || 0;
    const firstAudio = Math.max(0, media.audio.findIndex((a) => a.default));
    let audio = firstAudio;
    let forceConvert = false;
    let subtitle = defaultSubtitle(media);
    let subs = null; // the text subtitles being drawn
    let mode = plan(media, audio, forceConvert, subtitle);
    let loads = 0; // so a slow load doesn't override a newer one
    let offset = 0; // where the current stream started (streams begin at 0)
    let pendingSeek = null; // target while Left/Right are being pressed
    let seekTimer, saveTimer, hideTimer;
    let destroyed = false;
    let started = false;
    const session = { start: Math.floor(Date.now() / 1000), recorded: false, completed: false };

    const video = h("video", { playsinline: true, preload: "auto" });
    // Copied streams start at the keyframe before the requested time, so this can run a
    // few seconds ahead; keep it within the file.
    const position = () => {
      const pos = pendingSeek !== null ? pendingSeek : mode.direct ? video.currentTime : offset + video.currentTime;
      return duration ? Math.min(pos, duration) : pos;
    };

    // Controls
    const playButton = h("button", { class: "pbtn", "aria-label": "Pause" }, "❚❚");
    const time = h("span", { class: "time" }, "0:00");
    const range = h("input", { type: "range", min: 0, max: Math.max(1, Math.floor(duration)), step: 1, value: 0, "aria-label": "Position" });
    const audioSelect = media.audio.length > 1
      ? h("select", { "aria-label": "Audio" }, media.audio.map((a, i) => h("option", { value: i, selected: i === audio }, trackLabel(a, i))))
      : null;
    const subtitleSelect = media.subtitles.length
      ? h("select", { "aria-label": "Subtitles" },
        h("option", { value: "", selected: subtitle === null }, "Subtitles off"),
        media.subtitles.map((t, i) => h("option", { value: i, selected: i === subtitle }, subtitleLabel(t, i))))
      : null;
    const fullButton = h("button", { class: "pbtn", "aria-label": "Full screen" }, "⛶");
    const message = h("div", { class: "pmsg", hidden: true });
    const overlay = h("div", { class: "poverlay", hidden: true });
    const controls = h("div", { class: "pcontrols" },
      h("div", { class: "ptop" },
        h("button", { class: "pbtn", "aria-label": "Back", onclick: () => history.back() }, "‹"),
        h("div", { class: "ptitle" }, h("b", {}, info.title), info.episode ? h("span", {}, info.episode) : null)),
      h("div", { class: "pbottom" },
        range,
        h("div", { class: "prow" }, playButton, time, h("span", { class: "spacer" }), subtitleSelect, audioSelect, fullButton)));
    // Focusable itself, so remote keys reach the player once a prompt closes.
    const root = h("div", { class: "player", tabindex: -1 }, video, message, controls, overlay);

    function showMessage(text) {
      message.textContent = text || "";
      message.hidden = !text;
    }

    function updateTime() {
      const pos = position();
      time.textContent = `${clock(pos)} / ${clock(duration)}`;
      if (document.activeElement !== range) range.value = Math.floor(pos);
    }

    function showControls() {
      root.classList.remove("idle");
      clearTimeout(hideTimer);
      hideTimer = setTimeout(() => {
        if (!video.paused && overlay.hidden && !controls.contains(document.activeElement)) root.classList.add("idle");
      }, HIDE_CONTROLS_MS);
    }

    /** Draws the chosen text subtitles (picture ones are part of the stream). */
    function setupSubtitles() {
      subs?.destroy();
      subs = null;
      const t = subtitle !== null ? media.subtitles[subtitle] : null;
      if (!t || t.image) return;
      subs = new Subtitles.Track({
        video, library, file, index: t.index, fonts: info.fonts, cjkFont: info.cjk_font,
        timeOffset: mode.direct ? 0 : offset,
      });
      subs.need(position());
    }

    /** Starts playing from `at` seconds (a new stream unless playing the file directly). */
    async function load(at) {
      at = Math.max(0, Math.min(at, duration ? duration - 1 : at));
      const token = ++loads;
      pendingSeek = at;
      showMessage("Loading…");
      if (mode.direct) {
        pendingSeek = null;
        if (!video.getAttribute("src")) {
          video.src = `/media/${library}/${file}`;
          video.addEventListener("loadedmetadata", () => (video.currentTime = at), { once: true });
        } else {
          video.currentTime = at;
        }
        subs?.need(at);
      } else {
        // Copied video can only start at a keyframe: start exactly at the one before `at`,
        // so the clock and subtitles line up.
        let start = at;
        if (mode.video === "copy" && at > 0) {
          try {
            start = (await api(`/api/start/${library}/${file}?t=${at.toFixed(3)}`)).start;
          } catch (_) {}
        }
        if (token !== loads || destroyed) return;
        pendingSeek = null;
        offset = start;
        subs?.setTimeOffset(offset);
        subs?.need(at);
        const from = mode.video === "copy" && start > 0 ? start + 0.01 : start;
        const burn = mode.burn !== null ? `&burn=${mode.burn}` : "";
        video.src = `/stream/${library}/${file}?start=${from.toFixed(3)}&video=${mode.video}&audiomode=${mode.audio}&audio=${audio}${burn}`;
      }
      video.play().catch(() => {
        showMessage("");
        showControls();
      });
      started = true;
    }

    function seekBy(delta) {
      const target = Math.max(0, Math.min(position() + delta, duration ? duration - 1 : Infinity));
      if (mode.direct) {
        video.currentTime = target;
      } else {
        // Streams restart at each seek, so wait until the keys stop.
        pendingSeek = target;
        clearTimeout(seekTimer);
        seekTimer = setTimeout(() => load(target), 600);
      }
      updateTime();
      showControls();
    }

    function togglePause() {
      if (video.paused) video.play().catch(() => {});
      else video.pause();
      showControls();
    }

    function setAudio(i) {
      if (i === audio) return;
      const at = position();
      audio = i;
      if (audioSelect) audioSelect.value = String(i);
      mode = plan(media, audio, forceConvert, subtitle);
      video.removeAttribute("src");
      load(at);
      toast(`Audio: ${trackLabel(media.audio[i], i)}`);
    }

    function setSubtitle(i) {
      if (i === subtitle) return;
      const at = position();
      subtitle = i;
      if (subtitleSelect) subtitleSelect.value = i === null ? "" : String(i);
      const next = plan(media, audio, forceConvert, subtitle);
      // Burning in (or no longer burning in) picture subtitles needs a new stream.
      const restart = next.direct !== mode.direct || next.video !== mode.video || next.burn !== mode.burn;
      mode = next;
      setupSubtitles();
      if (restart) {
        video.removeAttribute("src");
        load(at);
      }
      toast(i === null ? "Subtitles off" : `Subtitles: ${subtitleLabel(media.subtitles[i], i)}`);
    }

    async function save(end) {
      const pos = position();
      if (!started || !duration || pos <= 0) return;
      const body = JSON.stringify({
        library: Number(library), file: Number(file), position: pos, duration,
        end, session_start: session.start, recorded: session.recorded, completed: session.completed,
      });
      try {
        const res = await fetch("/api/progress", { method: "POST", headers: { "Content-Type": "application/json" }, body, keepalive: true });
        if (res.ok) {
          const r = await res.json();
          session.recorded ||= r.recorded;
          session.completed ||= r.watched;
        }
      } catch (_) {}
    }

    function upNext() {
      overlay.replaceChildren();
      if (!info.next) {
        history.back();
        return;
      }
      const play = h("button", { class: "btn primary", onclick: () => location.replace(`#/play/${library}/${info.next.file}`) }, "Play");
      overlay.append(
        h("div", { class: "pbox" },
          h("div", { class: "dim" }, "Up next"),
          h("h2", {}, info.next.title),
          h("div", { class: "actions" }, play, h("button", { class: "btn", onclick: () => history.back() }, "Back"))));
      overlay.hidden = false;
      root.classList.remove("idle");
      play.focus();
    }

    // Events
    video.addEventListener("playing", () => { showMessage(""); playButton.textContent = "❚❚"; showControls(); });
    video.addEventListener("pause", () => { playButton.textContent = "▶"; root.classList.remove("idle"); save(false); });
    video.addEventListener("waiting", () => showMessage("Loading…"));
    video.addEventListener("timeupdate", updateTime);
    video.addEventListener("ended", async () => {
      clearInterval(saveTimer);
      pendingSeek = duration;
      await save(false);
      upNext();
    });
    video.addEventListener("error", () => {
      if (destroyed || !video.getAttribute("src")) return;
      if (!forceConvert) {
        // The browser couldn't decode it after all: convert everything instead.
        const at = position();
        forceConvert = true;
        mode = plan(media, audio, true, subtitle);
        video.removeAttribute("src");
        load(at);
      } else {
        showMessage("Couldn't play this video.");
      }
    });
    playButton.addEventListener("click", togglePause);
    fullButton.addEventListener("click", toggleFullscreen);
    range.addEventListener("change", () => {
      const target = Number(range.value);
      if (mode.direct) video.currentTime = target;
      else load(target);
    });
    audioSelect?.addEventListener("change", () => setAudio(Number(audioSelect.value)));
    subtitleSelect?.addEventListener("change", () => setSubtitle(subtitleSelect.value === "" ? null : Number(subtitleSelect.value)));
    // Seeking within the file itself: make sure subtitles for there are coming.
    video.addEventListener("seeking", () => { if (mode.direct) subs?.need(video.currentTime); });
    root.addEventListener("pointermove", showControls);
    video.addEventListener("click", (e) => {
      if (e.pointerType === "touch" && root.classList.contains("idle")) showControls();
      else togglePause();
    });
    video.addEventListener("dblclick", toggleFullscreen);
    const onPageHide = () => save(true);
    addEventListener("pagehide", onPageHide);
    saveTimer = setInterval(() => { if (!video.paused) save(false); }, SAVE_EVERY_MS);

    function toggleFullscreen() {
      if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
      else root.requestFullscreen?.().catch(() => {});
    }

    // Resume where you left off?
    if (startAt === null && info.position) {
      const start = (at) => {
        overlay.hidden = true;
        root.focus({ preventScroll: true });
        pendingSeek = at;
        setupSubtitles();
        load(at);
      };
      const resume = h("button", { class: "btn primary", onclick: () => start(info.position) }, `Resume from ${clock(info.position)}`);
      overlay.append(
        h("div", { class: "pbox" },
          h("h2", {}, info.episode || info.title),
          h("div", { class: "actions" }, resume, h("button", { class: "btn", onclick: () => start(0) }, "Play from the beginning"))));
      overlay.hidden = false;
      setTimeout(() => resume.focus());
    } else {
      setTimeout(() => root.focus({ preventScroll: true }));
      pendingSeek = startAt || 0;
      setupSubtitles();
      load(startAt || 0);
    }
    updateTime();

    return {
      title,
      fullscreen: true,
      content: [root],
      /** Keys while the player is showing; true if handled. */
      onKey(e) {
        if (!overlay.hidden) return false; // arrow keys move between the overlay's buttons
        const onControl = document.activeElement && controls.contains(document.activeElement) && document.activeElement !== playButton;
        switch (e.key) {
          case "Enter":
          case " ":
          case "MediaPlayPause":
            if (onControl && e.key !== "MediaPlayPause") return false;
            togglePause();
            return true;
          case "ArrowLeft":
          case "ArrowRight":
            if (document.activeElement === range) return false; // the seek bar moves itself
            seekBy(e.key === "ArrowLeft" ? -SEEK_STEP : SEEK_STEP);
            return true;
          case "ArrowUp":
            if (media.audio.length > 1) setAudio((audio + 1) % media.audio.length);
            else showControls();
            return true;
          case "ArrowDown":
            if (media.subtitles.length) {
              // Off, then each track in turn.
              const next = subtitle === null ? 0 : subtitle + 1 < media.subtitles.length ? subtitle + 1 : null;
              setSubtitle(next);
            } else {
              showControls();
            }
            return true;
          case "f":
            toggleFullscreen();
            return true;
        }
        showControls();
        return false;
      },
      destroy() {
        destroyed = true;
        save(true);
        subs?.destroy();
        clearInterval(saveTimer);
        clearTimeout(seekTimer);
        clearTimeout(hideTimer);
        removeEventListener("pagehide", onPageHide);
        // Stop downloading (Flex stops ffmpeg when the stream is dropped).
        video.pause();
        video.removeAttribute("src");
        video.load();
        if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
      },
    };
  }

  return { page };
})();
