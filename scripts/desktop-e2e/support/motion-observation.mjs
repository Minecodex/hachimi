/* global document, MutationObserver */

// These callbacks are serialized into the current WebView by the driver.
export function beginAmbientObservation() {
  const canvas = document.querySelector(".pet-avatar-canvas");
  if (!canvas) throw new Error("Pet canvas is missing");
  const observation = { motionId: null, recovered: false, observer: null };
  const capture = () => {
    const id = canvas.getAttribute("data-motion-ambient");
    const action = canvas.getAttribute("data-motion-action-id");
    const slots = canvas.getAttribute("data-motion-slots");
    if (!observation.motionId && id && action === id && slots?.includes("action")) {
      observation.motionId = id;
    }
    if (observation.motionId && id === observation.motionId && slots === "base") {
      observation.recovered = true;
      observation.observer?.disconnect();
    }
  };
  observation.observer = new MutationObserver(capture);
  observation.observer.observe(canvas, {
    attributes: true,
    attributeFilter: ["data-motion-ambient", "data-motion-action-id", "data-motion-slots"],
  });
  window.__HACHIMI_AMBIENT_OBSERVATION__ = observation;
  capture();
}

export function readAmbientObservation() {
  return window.__HACHIMI_AMBIENT_OBSERVATION__?.motionId || false;
}

export function readAmbientRecovery() {
  return window.__HACHIMI_AMBIENT_OBSERVATION__?.recovered === true;
}

export function endAmbientObservation() {
  window.__HACHIMI_AMBIENT_OBSERVATION__?.observer?.disconnect();
  delete window.__HACHIMI_AMBIENT_OBSERVATION__;
}

// A one-shot action or short gaze response can complete between WebDriver
// polls. Observe real rendered frame attributes before dispatching the input.
export function beginMotionObservation(motionId) {
  const canvas = document.querySelector(".pet-avatar-canvas");
  const observation = { started: false, recovered: false, observer: null };
  const capture = () => {
    if (canvas.getAttribute("data-motion-action-id") === motionId) observation.started = true;
    if (observation.started && canvas.getAttribute("data-motion-slots") === "base") {
      observation.recovered = true;
    }
  };
  observation.observer = new MutationObserver(capture);
  observation.observer.observe(canvas, {
    attributes: true,
    attributeFilter: ["data-motion-action-id", "data-motion-slots"],
  });
  window.__HACHIMI_MOTION_OBSERVATION__?.observer?.disconnect();
  window.__HACHIMI_MOTION_OBSERVATION__ = observation;
}

export function readMotionObservation() {
  return window.__HACHIMI_MOTION_OBSERVATION__?.started === true;
}

export function readMotionRecovery() {
  return window.__HACHIMI_MOTION_OBSERVATION__?.recovered === true;
}

export function beginGazeObservation() {
  const canvas = document.querySelector(".pet-avatar-canvas");
  const observation = { yaw: 0, observer: null };
  observation.observer = new MutationObserver(() => {
    observation.yaw = Math.max(
      observation.yaw,
      Number(canvas.getAttribute("data-motion-head-yaw")),
    );
  });
  observation.observer.observe(canvas, {
    attributes: true,
    attributeFilter: ["data-motion-head-yaw"],
  });
  window.__HACHIMI_GAZE_OBSERVATION__?.observer?.disconnect();
  window.__HACHIMI_GAZE_OBSERVATION__ = observation;
}

export function readGazeObservation() {
  return window.__HACHIMI_GAZE_OBSERVATION__?.yaw ?? 0;
}
