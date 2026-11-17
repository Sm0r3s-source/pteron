/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

(() => {
  "use strict";

  // States.
  const BUFFERING = "buffering";
  const ENDED = "ended";
  const ERRORED = "errored";
  const PAUSED = "paused";
  const PLAYING = "playing";

  // State transitions.
  const TRANSITIONS = {
    buffer: {
      paused: BUFFERING
    },
    end: {
      playing: ENDED,
      paused: ENDED
    },
    error: {
      buffering: ERRORED,
      playing: ERRORED,
      paused: ERRORED
    },
    pause: {
      buffering: PAUSED,
      playing: PAUSED,
      // https://html.spec.whatwg.org/multipage/#playing-the-media-resource
      // > It is possible for a media element to have both ended playback
      // > and paused for user interaction at the same time.
      ended: ENDED,
    },
    play: {
      buffering: PLAYING,
      ended: PLAYING,
      paused: PLAYING
    }
  };

  // Sirocco: the control bar sits at the top of the video. The sliders are drawn by hand
  // (track, fill, knob); the real range inputs lie invisibly on top and keep working as before.
  function generateMarkup(isAudioOnly) {
    return `
      <div class="controls">
        <button id="play-pause-button"></button>
        <span id="position-duration-box" class="hidden">
          <span id="position-text">#1</span>
          <span id="duration"> / #2</span>
        </span>
        <div class="slider seek">
          <div class="track"><div class="fill" id="progress-fill"></div><div class="marks" id="progress-marks"></div><div class="knob" id="progress-knob"></div></div>
          <div class="seektip hidden" id="progress-tip"></div>
          <input id="progress" type="range" value="0" min="0" max="100" step="0.1"></input>
        </div>
        <button id="volume-switch"></button>
        <div class="slider vol">
          <div class="track"><div class="fill" id="volume-fill"></div><div class="knob" id="volume-knob"></div></div>
          <input id="volume-level" type="range" value="100" min="0" max="100" step="1"></input>
        </div>
        ${isAudioOnly ? "" : '<button id="fullscreen-switch" class="fullscreen"></button>'}
      </div>
    `;
  }

  function camelCase(str) {
    const rdashes = /-(.)/g;
    return str.replace(rdashes, (str, p1) => {
      return p1.toUpperCase();
    });
  }

  function formatTime(time, showHours = false) {
    // Sirocco: "m:ss" or "h:mm:ss"; "--:--" when the length is not known.
    if (!Number.isFinite(time) || time < 0) return "--:--";
    time = Math.round(time / 1000);

    const hours = Math.floor(time / 3600);
    const mins = Math.floor((time % 3600) / 60);
    const secs = Math.floor(time % 60);

    if (hours || showHours) {
      return `${hours}:${mins.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
    }
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  }

  class MediaControls {
    constructor() {
      this.nonce = Date.now();
      // Get the instance of the shadow root where these controls live.
      this.controls = document.servoGetMediaControls("@@@id@@@");
      // Get the instance of the host of these controls.
      this.media = this.controls.host;

      this.mutationObserver = new MutationObserver((mutations) => {
        for (const mutation of mutations) {
          // Sirocco: chapters and skipped parts for the seek bar (`data-marks` on the video).
          if (mutation.type === "attributes" && mutation.attributeName === "data-marks") {
            this.drawMarks();
            continue;
          }
          // Here we handle the `controls` attribute removal.
          if (mutation.type === "attributes") {
            if (this.media.hasAttribute('controls')) {
              this.initializeDom();
            } else {
              this.cleanupDom();
            }
            return;
          }
          // Here we handle element removal from DOM.
          if (mutation.type === "childList") {
            for (const node of mutation.removedNodes) {
              if (node === this.media) {
                this.cleanupDom();
                this.cleanupCompleteControls();
                return;
              }
            }
          }
        }
      });
      this.mutationObserver.observe(this.media, {
        attributeFilter: ["controls", "data-marks"]
      });
      if (this.media.parentNode) {
        this.mutationObserver.observe(this.media.parentNode, {
          childList: true
        });
      }

      this.isAudioOnly = this.media.localName == "audio";

      // We also render the shadowtree for cues, which means that the
      // controls scripts run, but we don't want to show anything.
      // The above mutation observer then ensures that the controls do
      // render if at a later stage these controls should be shown.
      if (this.media.hasAttribute('controls')) {
        this.initializeDom();
      }
    }

    initializeDom() {
      // Do not reinitialize twice
      if (this.root) {
        return;
      }

      // Create root element and load markup.
      this.root = document.createElement("div");
      this.root.classList.add("controls-container");
      this.root.innerHTML = generateMarkup(this.isAudioOnly);
      this.controls.firstChild.appendChild(this.root);


      const elementNames = [
        "duration",
        "play-pause-button",
        "position-duration-box",
        "position-text",
        "progress",
        "progress-fill",
        "progress-knob",
        "progress-marks",
        "progress-tip",
        "volume-switch",
        "volume-level",
        "volume-fill",
        "volume-knob"
      ];

      if (!this.isAudioOnly) {
        elementNames.push("fullscreen-switch");
      }

      // Import elements.
      this.elements = {};
      elementNames.forEach(id => {
        this.elements[camelCase(id)] = this.controls.getElementById(id);
      });

      // Init position duration box.
      const positionTextNode = this.elements.positionText;
      const durationSpan = this.elements.duration;
      const durationFormat = durationSpan.textContent;
      const positionFormat = positionTextNode.textContent;

      durationSpan.classList.add("duration");
      durationSpan.setAttribute("role", "none");

      Object.defineProperties(this.elements.positionDurationBox, {
        durationSpan: {
          value: durationSpan
        },
        position: {
          get: () => {
            return positionTextNode.textContent;
          },
          set: v => {
            positionTextNode.textContent = positionFormat.replace("#1", v);
          }
        },
        duration: {
          get: () => {
            return durationSpan.textContent;
          },
          set: v => {
            durationSpan.textContent = v ? durationFormat.replace("#2", v) : "";
          }
        },
        show: {
          value: (currentTime, duration) => {
            const self = this.elements.positionDurationBox;
            if (self.position != currentTime) {
              self.position = currentTime;
            }
            if (self.duration != duration) {
              self.duration = duration;
            }
            self.classList.remove("hidden");
          }
        }
      });

      // Add event listeners.
      this.mediaEvents = [
        "play",
        "pause",
        "ended",
        "volumechange",
        "loadeddata",
        "loadstart",
        "timeupdate",
        "progress",
        "playing",
        "waiting",
        "canplay",
        "canplaythrough",
        "seeking",
        "seeked",
        "emptied",
        "loadedmetadata",
        "durationchange",
        "error",
        "suspend"
      ];
      this.mediaEvents.forEach(event => {
        this.media.addEventListener(event, this);
      });

      this.controlEvents = [
        { el: this.elements.playPauseButton, type: "click" },
        { el: this.elements.volumeSwitch, type: "click" },
        { el: this.elements.volumeLevel, type: "input" },
        { el: this.elements.progress, type: "input" },
        { el: this.elements.progress, type: "change" }
      ];
      this.scrubbing = false;
      this.installRangeDrag(this.elements.progress, percent => this.scrubPercent(percent), () => {
        this.scrubbing = false;
        this.render();
      }, () => { this.scrubbing = true; });
      // Sirocco: the time (and chapter) under the mouse on the seek bar.
      this.elements.progress.addEventListener("mousemove", e => this.showSeekTip(e));
      this.elements.progress.addEventListener("mouseleave", () => this.elements.progressTip.classList.add("hidden"));
      this.drawMarks();
      this.installRangeDrag(this.elements.volumeLevel, percent => {
        this.elements.volumeLevel.value = percent;
        this.media.muted = false;
        this.media.volume = percent / 100;
      });
      this.elements.progress.addEventListener("keydown", e => {
        if (!e.isTrusted || !Number.isFinite(this.length)) return;
        let delta = 0;
        if (e.key === "ArrowRight" || e.key === "ArrowUp") delta = 5;
        else if (e.key === "ArrowLeft" || e.key === "ArrowDown") delta = -5;
        else return;
        e.preventDefault();
        this.media.currentTime = Math.max(0, Math.min(this.length, this.media.currentTime + delta));
      });

      if (!this.isAudioOnly) {
        this.controlEvents.push({ el: this.elements.fullscreenSwitch, type: "click" });
      }

      this.controlEvents.forEach(({ el, type }) => {
        el.addEventListener(type, this);
      });

      // Sirocco: hide the bar while a video plays and the mouse is idle.
      this.idleTimer = null;
      this.setCursorHidden = hide => {
        if (hide && this.maximized) {
          this.media.style.setProperty("cursor", "none", "important");
        } else {
          this.media.style.removeProperty("cursor");
        }
      };
      this.wake = () => {
        this.root.classList.remove("idle");
        this.setCursorHidden(false);
        clearTimeout(this.idleTimer);
        this.idleTimer = setTimeout(() => {
          if (this.state == PLAYING && !this.scrubbing) {
            this.root.classList.add("idle");
            // In fullscreen the mouse arrow goes away with the bar, like on YouTube.
            this.setCursorHidden(true);
          }
        }, 2600);
      };
      ["mousemove", "mousedown", "keydown"].forEach(name => {
        this.media.addEventListener(name, this.wake);
        this.controls.addEventListener(name, this.wake);
      });

      // Sirocco: leave maximized mode when the window leaves real fullscreen (Esc, etc.).
      this.onFullscreenChange = () => {
        if (document.fullscreenElement) {
          this.sawFullscreen = true;
        } else if (this.maximized && this.sawFullscreen) {
          this.exitMaximize();
        }
      };
      document.addEventListener("fullscreenchange", this.onFullscreenChange);

      // Create state transitions.
      //
      // It exposes one method per transition. i.e. this.pause(), this.play(), etc.
      // For each transition, we check that the transition is possible and call
      // the `onStateChange` handler.
      for (let name in TRANSITIONS) {
        if (!TRANSITIONS.hasOwnProperty(name)) {
          continue;
        }
        this[name] = () => {
          const from = this.state;

          // Checks if the transition is valid in the current state.
          if (!TRANSITIONS[name][from]) {
            const error = `Transition "${name}" invalid for the current state "${from}"`;
            console.error(error);
            throw new Error(error);
          }

          const to = TRANSITIONS[name][from];

          if (from == to) {
            return;
          }

          // Transition to the next state.
          this.state = to;
          this.onStateChange(from);
        };
      }

      // Set initial state.
      this.state = this.media.paused ? PAUSED : PLAYING;
      this.onStateChange(null);
    }

    cleanupDom() {
      document.removeEventListener("fullscreenchange", this.onFullscreenChange);
      clearTimeout(this.idleTimer);
      if (!this.root) {
        return;
      }
      this.mediaEvents.forEach(event => {
        this.media.removeEventListener(event, this);
      });
      this.controlEvents.forEach(({ el, type }) => {
        el.removeEventListener(type, this);
      });
      this.root.remove();
      this.root = null;
    }

    cleanupCompleteControls() {
      this.mutationObserver.disconnect();
      this.mutationObserver = null;
      this.media = null;
      this.controls = null;
    }

    // State change handler
    onStateChange(from) {
      this.render(from);
      if (this.wake) this.wake();
    }

    render(from = this.state) {
      // Error
      if (this.state == ERRORED) {
        // XXX render errored state
        return;
      }

      if (this.state != from) {
        // Play/Pause button.
        const playPauseButton = this.elements.playPauseButton;
        playPauseButton.classList.remove(from);
        playPauseButton.classList.add(this.state);
      }

      // Progress.
      const positionPercent =
        (this.media.currentTime / this.length) * 100;
      if (this.scrubbing) {
        // The user is dragging the progress bar; do not fight their thumb.
      } else if (Number.isFinite(positionPercent)) {
        this.elements.progress.value = positionPercent;
      } else {
        this.elements.progress.value = 0;
      }

      // Current time and duration.
      let currentTime = formatTime(0);
      let duration = formatTime(NaN);
      if (!isNaN(this.media.currentTime)) {
        currentTime = formatTime(Math.round(this.media.currentTime * 1000));
        duration = formatTime(Math.round(this.length * 1000));
      }
      this.elements.positionDurationBox.show(currentTime, duration);

      // Volume.
      this.elements.volumeSwitch.className =
        this.media.muted || !this.media.volume ? "muted" : "volumeup";
      const volumeLevelValue = this.media.muted
        ? 0
        : Math.round(this.media.volume * 100);
      if (this.elements.volumeLevel.value != volumeLevelValue) {
        this.elements.volumeLevel.value = volumeLevelValue;
      }
      this.syncSliders();
    }

    // Sirocco: chapter lines and skipped parts (SponsorBlock) on the seek bar. The page puts them
    // on the video as `data-marks`: [{"s": start, "e": end (parts only), "t": title, "k": "ch" | "sb"}].
    readMarks() {
      try {
        const marks = JSON.parse(this.media.getAttribute("data-marks") || "[]");
        return Array.isArray(marks) ? marks.slice(0, 400) : [];
      } catch (e) {
        return [];
      }
    }

    drawMarks() {
      const box = this.elements.progressMarks;
      if (!box) return;
      box.textContent = "";
      const length = this.length;
      if (!Number.isFinite(length) || length <= 0) return;
      for (const mark of this.readMarks()) {
        const start = Number(mark.s);
        if (!Number.isFinite(start) || start < 0 || start > length) continue;
        const piece = document.createElement("div");
        if (mark.k === "sb") {
          const end = Math.min(length, Number(mark.e));
          if (!(end > start)) continue;
          piece.className = "mark-part";
          piece.style.left = (start / length) * 100 + "%";
          piece.style.width = ((end - start) / length) * 100 + "%";
        } else if (start > 0) {
          piece.className = "mark-chapter";
          piece.style.left = (start / length) * 100 + "%";
        } else {
          continue;
        }
        box.appendChild(piece);
      }
    }

    // Sirocco: while pointing at the seek bar, show the time (and chapter) there.
    showSeekTip(event) {
      const tip = this.elements.progressTip;
      const slider = this.elements.progress;
      const length = this.length;
      if (!tip || !slider || !Number.isFinite(length) || length <= 0) return;
      const rect = slider.getBoundingClientRect();
      if (!rect.width) return;
      const fraction = Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width));
      const at = fraction * length;
      let title = "";
      for (const mark of this.readMarks()) {
        if (mark.k === "sb" && at >= mark.s && at < mark.e) {
          title = mark.t;
          break;
        }
        if (mark.k !== "sb" && at >= mark.s) {
          title = mark.t;
        }
      }
      tip.textContent = formatTime(at * 1000) + (title ? "  " + title : "");
      tip.style.left = fraction * 100 + "%";
      tip.classList.remove("hidden");
    }

    // Sirocco: move the drawn fill and knob to match the (invisible) range inputs.
    syncSliders() {
      const pairs = [
        [this.elements.progress, this.elements.progressFill, this.elements.progressKnob],
        [this.elements.volumeLevel, this.elements.volumeFill, this.elements.volumeKnob]
      ];
      for (const [input, fill, knob] of pairs) {
        if (!input || !fill || !knob) continue;
        let p = parseFloat(input.value);
        if (!Number.isFinite(p)) p = 0;
        p = Math.max(0, Math.min(100, p));
        fill.style.width = p + "%";
        knob.style.left = p + "%";
      }
    }

    handleEvent(event) {
      if (!event.isTrusted) {
        console.warn(`Drop untrusted event ${event.type}`);
        return;
      }

      if (this.mediaEvents.includes(event.type)) {
        this.onMediaEvent(event);
      } else {
        this.onControlEvent(event);
      }
    }

    onControlEvent(event) {
      switch (event.type) {
        case "click":
          switch (event.currentTarget) {
            case this.elements.playPauseButton:
              this.playOrPause();
              break;
            case this.elements.volumeSwitch:
              this.toggleMuted();
              break;
            case this.elements.fullscreenSwitch:
                this.toggleFullscreen();
                break;
          }
          break;
        case "input":
          switch (event.currentTarget) {
            case this.elements.volumeLevel:
              this.changeVolume();
              break;
            case this.elements.progress:
              this.scrubTo();
              break;
          }
          break;
        case "change":
          if (event.currentTarget === this.elements.progress) {
            this.scrubTo();
            this.scrubbing = false;
            this.render();
          }
          break;
        default:
          throw new Error(`Unknown event ${event.type}`);
      }
    }

    // HTMLMediaElement event handler
    onMediaEvent(event) {
      switch (event.type) {
        case "ended":
          this.end();
          break;
        case "play":
        case "pause":
          // Transition to PLAYING or PAUSED state.
          this[event.type]();
          break;
        case "volumechange":
        case "timeupdate":
        case "resize":
          this.render();
          break;
        case "durationchange":
          this.render();
          this.drawMarks();
          break;
        case "loadedmetadata":
          this.drawMarks();
          break;
      }
    }

    /* Media actions */

    playOrPause() {
      switch (this.state) {
        case PLAYING:
          this.media.__userPaused = Date.now();
          this.media.pause();
          break;
        case BUFFERING:
        case ENDED:
        case PAUSED:
          this.media.play();
          break;
        default:
          throw new Error(`Invalid state ${this.state}`);
      }
    }

    toggleMuted() {
      this.media.muted = !this.media.muted;
    }

    // Sirocco: real fullscreen. Servo's own top-layer fullscreen draws a video offset, so the
    // video is "maximized" with fixed positioning to fill the page, and the whole *document*
    // is asked for fullscreen (not the video), which makes the window itself go fullscreen.
    toggleFullscreen() {
      const media = this.media;
      // The page ignores the click that pressed this button (the video has just moved under
      // the mouse, so it would count as a click on the picture and pause it).
      media.dispatchEvent(new CustomEvent("pteron-fullscreen"));
      const wasPlaying = !media.paused && !media.ended;
      const toggledAt = Date.now();
      if (wasPlaying) {
        // If anything pauses the video during the switch, carry on playing.
        [250, 700, 1500].forEach(ms => setTimeout(() => {
          if (media.paused && !media.ended && !(media.__userPaused > toggledAt)) {
            const p = media.play();
            if (p && p.catch) p.catch(() => {});
          }
        }, ms));
      }
      if (this.maximized) {
        this.exitMaximize();
        return;
      }
      this.maximized = true;
      this.savedStyle = media.getAttribute("style");
      const fixed = [
        "position:fixed", "top:0", "left:0", "right:0", "bottom:0",
        "width:100vw", "height:100vh", "max-width:none", "max-height:none",
        "margin:0", "padding:0", "border:0", "z-index:2147483647",
        "background:#000", "object-fit:contain"
      ].join("!important;") + "!important";
      media.setAttribute("style", (this.savedStyle ? this.savedStyle + ";" : "") + fixed);
      this.elements.fullscreenSwitch.classList.add("fullscreen-active");
      this.escHandler = e => {
        if (e.key === "Escape") {
          this.exitMaximize();
        }
      };
      document.addEventListener("keydown", this.escHandler, true);
      window.addEventListener("keydown", this.escHandler, true);
      this.askedFullscreen = false;
      this.sawFullscreen = false;
      try {
        const root = document.documentElement;
        if (root && root.requestFullscreen) {
          const result = root.requestFullscreen();
          this.askedFullscreen = true;
          if (result && result.catch) {
            result.catch(() => {
              this.askedFullscreen = false;
            });
          }
        }
      } catch (e) {
        this.askedFullscreen = false;
      }
    }

    exitMaximize() {
      if (!this.maximized) return;
      this.maximized = false;
      if (this.savedStyle === null || this.savedStyle === undefined) {
        this.media.removeAttribute("style");
      } else {
        this.media.setAttribute("style", this.savedStyle);
      }
      this.elements.fullscreenSwitch.classList.remove("fullscreen-active");
      document.removeEventListener("keydown", this.escHandler, true);
      window.removeEventListener("keydown", this.escHandler, true);
      if (this.askedFullscreen && document.fullscreenElement) {
        try {
          document.exitFullscreen();
        } catch (e) {}
      }
      this.askedFullscreen = false;
    }

    // Servo's <input type=range> is display-only (no mouse/keyboard handling),
    // so implement click and drag here.
    installRangeDrag(el, onValue, onEnd, onStart) {
      const percentFor = event => {
        const rect = el.getBoundingClientRect();
        if (!rect.width) return null;
        const p = ((event.clientX - rect.left) / rect.width) * 100;
        return Math.max(0, Math.min(100, p));
      };
      let dragging = false;
      const move = event => {
        if (!dragging) return;
        const p = percentFor(event);
        if (p !== null) {
          el.value = p;
          onValue(p);
          this.syncSliders();
        }
      };
      const up = event => {
        if (!dragging) return;
        dragging = false;
        document.removeEventListener("mousemove", move, true);
        document.removeEventListener("mouseup", up, true);
        move(event);
        if (onEnd) onEnd();
      };
      el.addEventListener("mousedown", event => {
        if (!event.isTrusted || event.button !== 0) return;
        dragging = true;
        if (onStart) onStart();
        document.addEventListener("mousemove", move, true);
        document.addEventListener("mouseup", up, true);
        move(event);
        event.preventDefault();
      });
    }

    // Sirocco: the real length; when the engine cannot tell (it reports Infinity), the page
    // can give it in a data-duration attribute.
    get length() {
      const d = this.media.duration;
      const fromPage = parseFloat(this.media.getAttribute("data-duration"));
      const page = Number.isFinite(fromPage) && fromPage > 0 ? fromPage : NaN;
      // While only the first part of a long video is loaded, show the whole length.
      if (this.media.hasAttribute("data-partial") && Number.isFinite(page)) return page;
      if (Number.isFinite(d) && d > 0) return d;
      return page;
    }

    // Seeking past the loaded first part tells the page, which loads the rest and jumps there.
    seekTo(time) {
      const d = this.media.duration;
      if (this.media.hasAttribute("data-partial") && Number.isFinite(d) && time > d - 1) {
        this.media.dispatchEvent(new CustomEvent("pteron-seek", { detail: time }));
      }
      this.media.currentTime = time;
    }

    scrubPercent(percent) {
      const duration = this.length;
      if (!Number.isFinite(duration) || duration <= 0) return;
      this.scrubbing = true;
      this.seekTo((percent / 100) * duration);
    }

    scrubTo() {
      const duration = this.length;
      const percent = parseFloat(this.elements.progress.value);
      if (!Number.isFinite(duration) || duration <= 0 || isNaN(percent)) {
        return;
      }
      this.scrubbing = true;
      clearTimeout(this.scrubTimer);
      // Safety net in case no "change" event follows (keyboard, wheel).
      this.scrubTimer = setTimeout(() => {
        this.scrubbing = false;
        this.render();
      }, 400);
      this.seekTo((percent / 100) * duration);
    }

    changeVolume() {
      const volume = parseInt(this.elements.volumeLevel.value);
      if (!isNaN(volume)) {
        this.media.volume = volume / 100;
      }
    }
  }

  new MediaControls();
})();

