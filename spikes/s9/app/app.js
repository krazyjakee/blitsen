// Four things this file has to make visible in the frame, because each one is a
// layer the spike is asking about and a blank window cannot tell them apart.
//
//  1. That a JavaScript engine ran at all — the fill below appears nowhere in
//     index.html or style.css, so an exact-RGB match for it in the captured
//     frame cannot come from anywhere else.
//  2. That it is a real one — IEEE 754 double addition printed to 17 places is
//     not something a stub returns by accident.
//  3. That text was measured, not merely drawn — the two widths come from
//     getBoundingClientRect(), so they are the layout's own numbers.
//  4. That two different system font families were resolved — the same string
//     in sans-serif and in monospace has to come out at two different widths.
//     One fallback for both, or no fonts at all, makes them equal.

// JS-SWATCH
var swatchFill = 'rgb(255, 0, 170)';

document.getElementById('js-swatch').style.backgroundColor = swatchFill;

document.getElementById('float').textContent = (0.1 + 0.2).toFixed(17);

function inkWidth(id) {
  var box = document.getElementById(id).getBoundingClientRect();
  return box.width.toFixed(2);
}

document.getElementById('sans-width').textContent = inkWidth('sans');
document.getElementById('mono-width').textContent = inkWidth('mono');
document.getElementById('viewport').textContent =
  window.innerWidth + ' x ' + window.innerHeight;

// One painted frame says a loop ran once. Two captures taken seconds apart, with
// this counter and this bar between them, say it is still running -- which is a
// different claim and the one the spike is actually making.
var frames = 0;
var counter = document.getElementById('frames');
var ticker = document.getElementById('ticker');

function tick() {
  frames += 1;
  counter.textContent = String(frames);
  ticker.style.width = (frames % 300) + 'px';
  requestAnimationFrame(tick);
}

requestAnimationFrame(tick);
