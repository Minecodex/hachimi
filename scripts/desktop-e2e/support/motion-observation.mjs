/* global document, MutationObserver */

// These callbacks are serialized into the current WebView by the driver.
export function beginAmbientObservation() {
  const canvas = document.querySelector(".pet-avatar-canvas");
  if (!canvas) throw new Error("Pet canvas is missing");
  const observation = { motionId: null, recovered: false, observer: null };
  const capture = () => {
    const id = canvas.getAttribute("data-motion-ambient");
    const slots = canvas.getAttribute("data-motion-slots");
    if (!observation.motionId && id && slots?.includes("action")) observation.motionId = id;
    if (observation.motionId && id === observation.motionId && slots === "base") {
      observation.recovered = true;
      observation.observer?.disconnect();
    }
  };
  observation.observer = new MutationObserver(capture);
  observation.observer.observe(canvas, {
    attributes: true,
    attributeFilter: ["data-motion-ambient", "data-motion-slots"],
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
