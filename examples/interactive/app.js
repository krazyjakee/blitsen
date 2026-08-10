const demo = document.getElementById("demo");
const stage = document.getElementById("stage");
const control = document.getElementById("control");
const controlLabel = document.getElementById("control-label");
const orb = document.getElementById("orb");
const inputState = document.getElementById("input-state");
const eventOrder = document.getElementById("event-order");
const layoutState = document.getElementById("layout-state");

let expanded = false;
let direction = 1;
let speed = 0.16;
let propagation = [];
let travel = 500;
let position = 0;
let previousTimestamp = null;

const measureTrack = () => {
  travel = Math.max(0, stage.clientWidth - 114);
  position = Math.min(position, travel);
};

const record = (name, capture = false) => event => {
  propagation.push(`${name}${capture ? "↓" : "↑"}`);
  if (name === "window" && !capture) {
    eventOrder.textContent = `${event.type}: ${propagation.join("  ")}`;
    demo.classList.add("has-event");
    propagation = [];
  }
};

for (const type of ["click", "keydown"]) {
  window.addEventListener(type, record("window", true), true);
  document.addEventListener(type, record("document", true), true);
  stage.addEventListener(type, record("stage", true), true);
  stage.addEventListener(type, record("stage"));
  document.addEventListener(type, record("document"));
  window.addEventListener(type, record("window"));
}

control.addEventListener("click", () => {
  expanded = !expanded;
  demo.classList.toggle("expanded", expanded);
  controlLabel.textContent = expanded ? "Click to compact" : "Click to expand";
  inputState.textContent = "click dispatched";
  layoutState.textContent = `${expanded ? "expanded" : "compact"} · ${demo.offsetWidth}px`;
  measureTrack();
});

control.addEventListener("keydown", event => {
  if (!["ArrowLeft", "ArrowRight", " "].includes(event.key)) return;
  event.preventDefault();
  if (event.key === "ArrowLeft") direction = -1;
  if (event.key === "ArrowRight") direction = 1;
  if (event.key === " ") speed = speed === 0 ? 0.16 : 0;
  demo.classList.remove("moving-left", "moving-right", "paused");
  demo.classList.add(speed === 0 ? "paused" : direction < 0 ? "moving-left" : "moving-right");
  inputState.textContent = event.key === " " ? (speed ? "animation resumed" : "animation paused")
    : `${event.key} dispatched`;
});

window.addEventListener("load", measureTrack);
window.addEventListener("resize", measureTrack);

const animate = timestamp => {
  const elapsed = previousTimestamp === null ? 0 : Math.min(50, timestamp - previousTimestamp);
  previousTimestamp = timestamp;
  position += elapsed * speed * direction;
  if (position >= travel) { position = travel; direction = -1; }
  if (position <= 0) { position = 0; direction = 1; }
  orb.style.left = `${8 + position}px`;
  requestAnimationFrame(animate);
};

demo.classList.add("moving-right");
demo.setAttribute("data-ready", "true");
requestAnimationFrame(animate);
