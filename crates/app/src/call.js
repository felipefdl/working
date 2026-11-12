// Runs inside the Meet page of a call window, before Meet's own code, on every page load.
//
// It strips the page down to the video tiles, joins by itself, keeps the camera away from Meet until the app's
// camera switch is on, clicks Meet's own buttons on behalf of the app's control bar, reports the call state
// back through `window.ipc`, and sends a shared screen sharper than WebKit's defaults give.
//
// Meet's class names are generated and change; everything here hooks on what stays put: the `main` element that
// holds the tiles, `jsname` attributes, and (English, forced with `hl=en`) accessibility labels.
(() => {
  if (window.__working) return;

  const CSS = `
    html.working-min { background: #131314 !important; }

    /* One line of status text: over the whole page before the call (the lobby is hidden behind it), and in
       the middle of the call area when there is no tile to show. */
    html.working-min.working-lobby body { visibility: hidden !important; }
    #working-status { display: none; }
    html.working-min.working-lobby #working-status,
    html.working-min.working-call.working-empty #working-status {
      display: flex; position: fixed; inset: 0; z-index: 2147483647; visibility: visible;
      align-items: center; justify-content: center; color: #9aa0a6; pointer-events: none;
      font: 500 15px -apple-system, system-ui, sans-serif;
    }
    html.working-min.working-lobby #working-status { background: #131314; }

    /* In the call: the tile area covers the window and lays its tiles out itself. Meet positions each tile
       with inline left/top/width/height computed for its own chrome; those are overridden. Everything else
       Meet puts in that area (the floating self view, pop-ups) and every control it draws on a tile is
       hidden: the app's own bar has the controls, and nothing sits on top of a picture. */
    html.working-min.working-call main {
      inset: 0 !important; z-index: 2147483000 !important; background: #131314 !important;
      display: flex !important; flex-wrap: wrap; align-content: stretch; gap: 6px; padding: 6px;
      box-sizing: border-box;
    }
    html.working-min.working-call main > div[style*="left:"] {
      position: relative !important; left: auto !important; top: auto !important;
      width: auto !important; height: auto !important; flex: 1 1 360px; min-width: 0; min-height: 0;
    }
    html.working-min.working-call main > :not([style*="left:"]) { display: none !important; }
    html.working-min.working-call main button,
    html.working-min.working-call main [role="button"] { display: none !important; }
    html.working-min.working-call [role="dialog"],
    html.working-min.working-call [aria-modal="true"] { display: none !important; }

    /* The tile of the user's own share only says that they are presenting. */
    html.working-min.working-call main > div[data-working="own-share"] { display: none !important; }

    /* Somebody presents: their screen takes the area, the other tiles a narrow column beside it. */
    html.working-min.working-call.working-stage main {
      flex-direction: column; flex-wrap: nowrap; align-items: flex-end; justify-content: flex-start;
    }
    html.working-min.working-call.working-stage main > div[style*="left:"] {
      flex: 0 0 96px; width: 168px !important;
    }
    html.working-min.working-call.working-stage main > div[data-working="stage"] {
      position: absolute !important; left: 6px !important; top: 6px !important; right: 180px; bottom: 6px;
      width: auto !important; height: auto !important;
    }
    html.working-min.working-call.working-stage.working-stage-alone main > div[data-working="stage"] { right: 6px; }
  `;

  let custom = true;
  let joined = false;
  let joinClickedAt = 0;
  // The camera's master switch, held by the app's camera button. While it is off, a camera request from Meet
  // is answered with a blank picture, so the real camera is never opened.
  let cameraAllowed = false;
  let cameraNudged = 0;
  const cameraTracks = new Set();
  let blankCanvas = null;
  let sent = '';
  let summary = '';
  let tilesSeen = '';
  // How loud the microphone is right now, 0 (silent or muted) to 4: the app draws it on its mic button, in
  // place of the self view Meet floats over the call.
  let voice = 0;
  let voiceMeter = null;
  const started = Date.now();

  const label = (el) => (el && el.getAttribute('aria-label')) || '';
  const one = (selector) => document.querySelector(selector);
  const toggle = (jsname, word) =>
    one(`button[data-is-muted][jsname="${jsname}"]`) ||
    [...document.querySelectorAll('button[data-is-muted]')].find((b) => label(b).toLowerCase().includes(word));
  const mic = () => toggle('hw0c9', 'microphone');
  const cam = () => toggle('psRWwc', 'camera');
  const leave = () => one('button[aria-label="Leave call"]');
  const share = () => one('button[jsname="hNGZQc"]') || one('button[aria-label="Share screen"]');
  const hand = () => one('button[jsname="FpSaz"]') || one('button[aria-label^="Raise hand"], button[aria-label^="Lower hand"]');
  // `textContent`, not `innerText`: the lobby is hidden while this runs, and hidden elements have no inner text.
  const byText = (pattern) => [...document.querySelectorAll('button')].find((b) => pattern.test(b.textContent.trim()));
  // A device toggle is on exactly when its label offers to turn it off; a missing device reads as unknown.
  const isOn = (el) => (!el ? '-' : /^turn off/i.test(label(el)) ? '1' : /^turn on/i.test(label(el)) ? '0' : '-');
  const click = (el) => el && el.click();
  const stopPresenting = () => byText(/^Stop presenting$/);

  function install() {
    const root = document.documentElement;
    if (!root) return false;
    if (!window.__workingSheet) {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(CSS);
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
      window.__workingSheet = sheet;
    }
    if (!document.getElementById('working-status')) {
      const status = document.createElement('div');
      status.id = 'working-status';
      status.textContent = 'Joining…';
      root.appendChild(status);
    }
    return true;
  }

  function tick() {
    if (!install()) return;
    const root = document.documentElement;
    const inCall = !!leave();
    if (inCall) joined = true;
    const phase = inCall ? 'call' : joined ? 'ended' : 'lobby';

    if (phase === 'lobby' && custom) {
      const allow = byText(/^Allow microphone and camera$/);
      // Also "Join now without camera", which Meet shows when it has no camera to offer.
      const join = byText(/^(Join now|Ask to join|Switch here)/);
      if (allow) {
        click(allow);
      } else if (join && Date.now() - joinClickedAt > 2500) {
        // Clicked again while the lobby is still up: an early click can land before Meet is ready for it.
        joinClickedAt = Date.now();
        click(join);
      }
      // Nothing to join automatically (a blocked meeting, an error page): show Google's page as it is.
      if (!joinClickedAt && Date.now() - started > 20000) custom = false;
    }
    // With the switch off, Meet's own camera toggle is kept off too, so nobody sees a black tile. Meet's
    // label lags a click, hence the pause between tries.
    if (!cameraAllowed && isOn(cam()) === '1' && Date.now() - cameraNudged > 1500) {
      cameraNudged = Date.now();
      click(cam());
    }

    // Tiles that are a shared screen: somebody else's goes on stage, the user's own is not shown to them.
    let staged = 0;
    let beside = 0;
    if (phase === 'call') {
      const tiles = [...document.querySelectorAll('main > div[style*="left:"]')];
      for (const tile of tiles) {
        const text = tile.textContent;
        const own = /you are presenting|\(your presentation\)/i.test(text);
        const kind = own ? 'own-share' : /presentation|presenting/i.test(text) ? 'stage' : 'person';
        if (tile.dataset.working !== kind) tile.dataset.working = kind;
        if (kind === 'stage') staged += 1;
        if (kind === 'person') beside += 1;
      }
      if (window.__workingTrace) {
        const seen = tiles.map((tile) => `${tile.dataset.working}: ${tile.textContent.trim().slice(0, 80)}`).join(' || ');
        if (seen !== tilesSeen) {
          tilesSeen = seen;
          window.ipc.postMessage('stats|' + JSON.stringify({ tiles: seen }));
        }
      }
    }

    const sharing = !!stopPresenting();
    root.classList.toggle('working-min', custom);
    root.classList.toggle('working-lobby', phase === 'lobby');
    root.classList.toggle('working-call', phase === 'call');
    root.classList.toggle('working-stage', staged > 0);
    root.classList.toggle('working-stage-alone', staged > 0 && beside === 0);
    root.classList.toggle('working-empty', phase === 'call' && staged + beside === 0);
    const status = document.getElementById('working-status');
    const note = phase === 'lobby' ? 'Joining…' : sharing ? 'You are sharing your screen' : 'Nobody else is here';
    if (status.textContent !== note) status.textContent = note;

    const raised = /^lower hand/i.test(label(hand())) ? '1' : '0';
    const camera = !cam() ? '-' : cameraAllowed && isOn(cam()) === '1' ? '1' : '0';
    const presenting = sharing ? '1' : '0';
    summary = [phase, isOn(mic()), camera, raised, custom ? '1' : '0', presenting].join('|');
    report();
  }

  // The state the app's window draws from: `phase|mic|camera|hand|custom|presenting|voice`.
  function report() {
    const state = `${summary}|${voice}`;
    if (summary && state !== sent) {
      sent = state;
      window.ipc.postMessage(state);
    }
  }

  window.__working = {
    setCustom(on) {
      custom = on;
      tick();
    },
    toggleMic: () => click(mic()),
    // The camera's master switch. Turning it on makes Meet ask for the camera again, this time for real.
    setCamera(on) {
      cameraAllowed = on;
      cameraNudged = Date.now();
      if ((isOn(cam()) === '1') !== on) click(cam());
      if (!on) {
        // Meet lets go of the camera when its toggle goes off; this makes sure of it.
        setTimeout(() => {
          cameraTracks.forEach((track) => track.stop());
          cameraTracks.clear();
        }, 500);
      }
      tick();
    },
    // While the user presents, the same button stops it.
    share: () => click(stopPresenting() || share()),
    toggleHand: () => click(hand()),
    leave: () => click(leave()),
  };

  // ---- Media quality -------------------------------------------------------------------------------------
  // Measured in a call: Meet asks WebKit for a screen of at most 1920x1200 at 5 fps, and WebKit answers that
  // with a 640-wide capture. Asking for the full size instead gets the display's size in points, which is the
  // most WebKit offers (it does not capture Retina pixels). The steps below ask for that, mark the stream as
  // text-like, and lift the bitrate cap.

  const SCREEN_BITRATE = 4000000;
  const screenTracks = new Set();
  const connections = [];
  const boosted = new WeakSet();
  const Devices = window.MediaDevices && MediaDevices.prototype;

  const sharpSize = () => {
    const scale = window.devicePixelRatio || 2;
    return {
      width: { ideal: Math.round(screen.width * scale) },
      height: { ideal: Math.round(screen.height * scale) },
      frameRate: { ideal: 30 },
    };
  };

  function adoptScreenTrack(track) {
    if (screenTracks.has(track.id)) return;
    screenTracks.add(track.id);
    // Text and UI: keep detail, drop frames instead.
    track.contentHint = 'detail';
    track.addEventListener('ended', (event) => {
      // Starting the native capture can end WebKit's own. While native pictures stand in for it, Meet must
      // not hear about that, or it would stop presenting. This listener is added before any of Meet's.
      if (feed) {
        if (window.__workingTrace) window.ipc.postMessage('stats|{"webkitCaptureEnded":"hidden from Meet"}');
        event.stopImmediatePropagation();
        return;
      }
      screenTracks.delete(track.id);
    });
  }

  // Patched on the prototype: Meet does not call the method through `navigator.mediaDevices`.
  if (Devices && Devices.getDisplayMedia) {
    const capture = Devices.getDisplayMedia;
    Devices.getDisplayMedia = async function (constraints = {}) {
      const video = typeof constraints.video === 'object' ? constraints.video : {};
      if (window.__workingTrace) window.ipc.postMessage('stats|' + JSON.stringify({ asked: constraints }));
      let stream;
      try {
        stream = await capture.call(this, { ...constraints, video: { ...video, ...sharpSize() } });
      } catch (error) {
        // The user cancelling the picker is final; anything else gets Meet's own request.
        if (error && error.name === 'NotAllowedError') throw error;
        stream = await capture.call(this, constraints);
      }
      stream.getVideoTracks().forEach(adoptScreenTrack);
      return stream;
    };
  }

  // Meet may send a copy of the captured track; the copy is the screen too.
  if (window.MediaStreamTrack) {
    const clone = MediaStreamTrack.prototype.clone;
    MediaStreamTrack.prototype.clone = function () {
      const copy = clone.call(this);
      if (screenTracks.has(this.id)) adoptScreenTrack(copy);
      return copy;
    };
  }

  function blankCameraTrack() {
    if (!blankCanvas) {
      blankCanvas = document.createElement('canvas');
      blankCanvas.width = 640;
      blankCanvas.height = 360;
      const context = blankCanvas.getContext('2d');
      const paint = () => {
        context.fillStyle = '#000';
        context.fillRect(0, 0, blankCanvas.width, blankCanvas.height);
      };
      paint();
      setInterval(paint, 1000);
    }
    return blankCanvas.captureStream(5).getVideoTracks()[0];
  }

  function meterVoice(stream) {
    const track = stream.getAudioTracks()[0];
    if (!track) return;
    try {
      if (voiceMeter) voiceMeter.context.close();
      const context = new AudioContext();
      const analyser = context.createAnalyser();
      analyser.fftSize = 512;
      context.createMediaStreamSource(new MediaStream([track])).connect(analyser);
      voiceMeter = { context, analyser, track, samples: new Uint8Array(analyser.fftSize) };
    } catch {
      voiceMeter = null;
    }
  }

  setInterval(() => {
    let level = 0;
    if (voiceMeter && voiceMeter.track.readyState === 'live' && isOn(mic()) === '1') {
      const { analyser, samples } = voiceMeter;
      analyser.getByteTimeDomainData(samples);
      let sum = 0;
      for (const sample of samples) sum += ((sample - 128) / 128) ** 2;
      // Speech sits around 0.05 to 0.2 on this scale.
      level = Math.min(4, Math.floor(Math.sqrt(sum / samples.length) * 25));
    }
    if (level !== voice) {
      voice = level;
      report();
    }
  }, 120);

  if (Devices && Devices.getUserMedia) {
    const open = Devices.getUserMedia;
    Devices.getUserMedia = async function (constraints = {}) {
      const wanted = { ...constraints };
      // Echo cancellation and noise suppression are WebKit's to do; a request never turns them off.
      if (wanted.audio) {
        const audio = typeof wanted.audio === 'object' ? wanted.audio : {};
        wanted.audio = { ...audio, echoCancellation: true, noiseSuppression: true };
      }
      if (!wanted.video || cameraAllowed) {
        const stream = await open.call(this, wanted);
        meterVoice(stream);
        if (wanted.video) stream.getVideoTracks().forEach((track) => cameraTracks.add(track));
        return stream;
      }
      // The camera switch is off: the microphone is real, the picture is blank.
      if (!wanted.audio) return new MediaStream([blankCameraTrack()]);
      const stream = await open.call(this, { ...wanted, video: false });
      meterVoice(stream);
      stream.addTrack(blankCameraTrack());
      return stream;
    };
  }

  const sharpen = (parameters) => {
    parameters.degradationPreference = 'maintain-resolution';
    for (const encoding of parameters.encodings || []) {
      // Only the full-size layer; the smaller simulcast layers are for slow receivers.
      if ((encoding.scaleResolutionDownBy || 1) <= 1) {
        encoding.maxBitrate = Math.max(encoding.maxBitrate || 0, SCREEN_BITRATE);
      }
    }
    return parameters;
  };

  if (window.RTCPeerConnection && window.RTCRtpSender) {
    window.RTCPeerConnection = new Proxy(window.RTCPeerConnection, {
      construct(target, args) {
        const connection = new target(...args);
        connections.push(connection);
        return connection;
      },
    });
    // Meet's own settings for the screen stream go through the same lift.
    const setParameters = RTCRtpSender.prototype.setParameters;
    RTCRtpSender.prototype.setParameters = function (parameters, ...rest) {
      const sharing = this.track && screenTracks.has(this.track.id);
      return setParameters.call(this, sharing ? sharpen(parameters) : parameters, ...rest);
    };
  }

  // A screen capture is labeled by what it shows ("Screen 1", a window title); a camera has a device name and
  // no display surface.
  const looksLikeScreen = (track) =>
    track.kind === 'video' && (!!track.getSettings().displaySurface || /^(screen|window)\b/i.test(track.label));

  // ---- Native screen feed ----------------------------------------------------------------------------------
  // WebKit's capture of a display stops at its size in points. When a whole display is shared, the app
  // captures its real pixels and sends the pictures here; they are drawn on a canvas whose stream takes the
  // place of WebKit's track on the wire. Meet keeps holding its own track, so its controls work as before.

  let feed = null;
  // The share the app could not capture natively (no permission): it stays on WebKit's capture.
  let refusedFor = null;
  const swapping = new WeakSet();

  const isDisplay = (track) => {
    const surface = track.getSettings().displaySurface;
    return surface ? surface === 'monitor' : /^screen\b/i.test(track.label);
  };

  function startFeed(track) {
    const size = track.getSettings();
    feed = { original: track, canvas: null, context: null, track: null, picture: null, idle: 0 };
    const kind = isDisplay(track) ? 'display' : 'window';
    window.ipc.postMessage(`capture|start|${kind}|${size.width}x${size.height}|${track.label}`);
  }

  function stopFeed() {
    if (feed.track) feed.track.stop();
    if (feed.picture) feed.picture.close();
    feed = null;
    window.ipc.postMessage('capture|stop');
  }

  // Redrawn once a second too, so a still screen keeps producing frames for people who join late.
  function paintFeed() {
    if (feed && feed.picture) feed.context.drawImage(feed.picture, 0, 0);
  }

  // Called by the app with each new picture of the shared display or window, a base64 JPEG.
  async function frame(base64) {
    if (!feed) return;
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    const picture = await createImageBitmap(new Blob([bytes], { type: 'image/jpeg' }));
    if (!feed) {
      picture.close();
      return;
    }
    if (!feed.canvas) {
      feed.canvas = document.createElement('canvas');
      feed.context = feed.canvas.getContext('2d');
      feed.track = feed.canvas.captureStream(10).getVideoTracks()[0];
      adoptScreenTrack(feed.track);
    }
    // A shared window can be resized while it is shared.
    if (feed.canvas.width !== picture.width || feed.canvas.height !== picture.height) {
      feed.canvas.width = picture.width;
      feed.canvas.height = picture.height;
    }
    if (feed.picture) feed.picture.close();
    feed.picture = picture;
    paintFeed();
  }
  window.__working.frame = frame;
  window.__working.captureRefused = () => {
    if (!feed) return;
    refusedFor = feed.original.id;
    feed = null;
  };

  function boostScreenSenders() {
    // The share is over once nothing sends the display any more, in either form.
    if (feed) {
      const carried = connections.some(
        (connection) =>
          connection.connectionState !== 'closed' &&
          connection.getSenders().some((s) => s.track && (s.track === feed.track || s.track.id === feed.original.id)),
      );
      feed.idle = carried ? 0 : feed.idle + 1;
      if (feed.idle >= 3) stopFeed();
    }
    for (const connection of connections) {
      if (connection.connectionState === 'closed') continue;
      for (const sender of connection.getSenders()) {
        const track = sender.track;
        if (!track) continue;
        // A capture that did not come through the wrapper above: take it over here and ask for full size.
        if (!screenTracks.has(track.id) && looksLikeScreen(track)) {
          adoptScreenTrack(track);
          track.applyConstraints(sharpSize()).catch(() => {});
        }
        if (!screenTracks.has(track.id)) continue;
        if (!feed && track.readyState === 'live' && track.id !== refusedFor) startFeed(track);
        // Once the first native picture is in, it goes out instead of WebKit's capture.
        if (feed && feed.track && track.id === feed.original.id && !swapping.has(sender)) {
          swapping.add(sender);
          sender.replaceTrack(feed.track).finally(() => swapping.delete(sender));
        }
        if (boosted.has(sender)) continue;
        boosted.add(sender);
        sender.setParameters(sender.getParameters()).catch(() => boosted.delete(sender));
      }
    }
  }

  // What the shared screen really is on its way out: capture size, encoder settings, and what the encoder
  // produces. The app prints these lines when started with `WORKING_TRACE=1`.
  const sentBytes = new Map();
  async function reportScreenStats() {
    if (!window.__workingTrace) return;
    let found = false;
    const seen = [];
    for (const connection of connections) {
      if (connection.connectionState === 'closed') continue;
      for (const sender of connection.getSenders()) {
        if (sender.track) {
          const s = sender.track.getSettings();
          seen.push(`${sender.track.kind}:${sender.track.label}:${s.width}x${s.height}`);
        }
        if (!sender.track || !screenTracks.has(sender.track.id)) continue;
        found = true;
        const capture = sender.track.getSettings();
        const encodings = (sender.getParameters().encodings || []).map((e) => ({
          rid: e.rid, active: e.active, max: e.maxBitrate, scale: e.scaleResolutionDownBy,
        }));
        const stats = await sender.getStats();
        const codecs = new Map();
        stats.forEach((s) => s.type === 'codec' && codecs.set(s.id, s.mimeType));
        const layers = [];
        stats.forEach((s) => {
          if (s.type !== 'outbound-rtp') return;
          const before = sentBytes.get(s.id) || s.bytesSent;
          sentBytes.set(s.id, s.bytesSent);
          layers.push({
            rid: s.rid, size: `${s.frameWidth}x${s.frameHeight}`, fps: s.framesPerSecond,
            kbps: Math.round(((s.bytesSent - before) * 8) / 2000), codec: codecs.get(s.codecId),
            limit: s.qualityLimitationReason,
          });
        });
        const limits = sender.track.getCapabilities ? sender.track.getCapabilities() : {};
        const report = {
          capture: `${capture.width}x${capture.height}@${Math.round(capture.frameRate || 0)}`,
          most: `${limits.width && limits.width.max}x${limits.height && limits.height.max}`,
          pixelRatio: window.devicePixelRatio, screen: `${screen.width}x${screen.height}`,
          hint: sender.track.contentHint, degrade: sender.getParameters().degradationPreference, encodings, layers,
        };
        window.ipc.postMessage('stats|' + JSON.stringify(report));
      }
    }
    if (!found) {
      const summary = { screenTracks: screenTracks.size, connections: connections.length, senders: seen };
      window.ipc.postMessage('stats|' + JSON.stringify(summary));
    }
  }

  setInterval(() => {
    tick();
    boostScreenSenders();
  }, 300);
  setInterval(() => reportScreenStats().catch(() => {}), 2000);
  setInterval(paintFeed, 1000);
})();
