// The program's audio, under the editor's control: every audio
// context the runtime makes sends its output through a gain node,
// whose level the editor sets (`setEaslVolume`, 0 to 1), and while a
// context waits for the visitor's first click (browsers start audio
// only then), the overlay asks for one. A context the page suspended
// to pause the program doesn't ask (`setEaslPaused`).
(() => {
  const contexts = [];
  let volume = 1;
  let paused = false;
  const overlay = document.getElementById("audio-overlay");
  const update = () => {
    overlay.hidden =
      paused || !contexts.some(({ context }) => context.state === "suspended");
  };
  const Native = window.AudioContext;
  const nativeDestination = Object.getOwnPropertyDescriptor(
    BaseAudioContext.prototype,
    "destination",
  ).get;
  window.AudioContext = class extends Native {
    constructor(...args) {
      super(...args);
      const gain = new GainNode(this, { gain: volume });
      gain.connect(nativeDestination.call(this));
      // The runtime connects its output to `destination`.
      Object.defineProperty(this, "destination", { value: gain });
      contexts.push({ context: this, gain });
      this.addEventListener("statechange", update);
      update();
    }
  };
  window.setEaslVolume = (level) => {
    volume = level;
    for (const { context, gain } of contexts) {
      gain.gain.setTargetAtTime(level, context.currentTime, 0.015);
    }
  };
  window.setEaslPaused = (isPaused) => {
    paused = isPaused;
    update();
  };
  overlay.addEventListener("click", () => {
    for (const { context } of contexts) context.resume();
    document.getElementById("canvas").focus();
  });
})();
