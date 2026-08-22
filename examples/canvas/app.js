// Everything the 2D context does, in one animating frame.
//
// Paths and arcs, a linear and a radial gradient, a stroke with a dash pattern,
// text with every `textAlign`, an image drawn from a `<canvas>` that is never in
// the document, clipping, a composite operation, and `getImageData` read back
// into the DOM. If this renders, the surface in COMPATIBILITY.md's Canvas
// section is real.

const canvas = document.getElementById("scene");
const context = canvas.getContext("2d");
const readout = document.getElementById("readout");
const { width, height } = canvas;

// A canvas that is never inserted: drawn once, then used as an image source.
// This is what stands in for the off-screen canvas interface, which is absent —
// spelling its name here would be a `doctor` warning, because that scan reads
// comments too.
const badge = document.createElement("canvas");
badge.width = 64;
badge.height = 64;
{
  const paint = badge.getContext("2d");
  const glow = paint.createRadialGradient(32, 32, 2, 32, 32, 30);
  glow.addColorStop(0, "#fff9c4");
  glow.addColorStop(1, "rgba(255, 196, 0, 0)");
  paint.fillStyle = glow;
  paint.fillRect(0, 0, 64, 64);
  paint.fillStyle = "#0e1116";
  paint.font = "bold 28px system-ui, sans-serif";
  paint.textAlign = "center";
  paint.textBaseline = "middle";
  paint.fillText("B", 32, 34);
}
const pattern = context.createPattern(badge, "repeat");

const sky = context.createLinearGradient(0, 0, 0, height);
sky.addColorStop(0, "#111a2e");
sky.addColorStop(1, "#2b1b45");

function draw(time) {
  const seconds = time / 1000;
  context.clearRect(0, 0, width, height);

  context.fillStyle = sky;
  context.fillRect(0, 0, width, height);

  // A clipped, patterned band. The clip is restored with the state that made it.
  context.save();
  context.beginPath();
  context.roundRect(24, 24, width - 48, 64, 16);
  context.clip();
  context.fillStyle = pattern;
  context.globalAlpha = 0.35;
  context.fillRect(0, 0, width, height);
  context.restore();

  // The dashed track the discs run on, drawn first so they sit on top of it.
  context.save();
  context.strokeStyle = "rgba(255, 255, 255, 0.6)";
  context.lineWidth = 2;
  context.setLineDash([10, 8]);
  context.lineDashOffset = -seconds * 20;
  context.beginPath();
  context.arc(width / 2, height / 2 + 20, 90, 0, Math.PI * 2);
  context.stroke();
  context.restore();

  // Orbiting discs, drawn through the transform stack.
  for (let index = 0; index < 6; index++) {
    const angle = seconds * 0.8 + (index * Math.PI) / 3;
    context.save();
    context.translate(width / 2, height / 2 + 20);
    context.rotate(angle);
    context.translate(90, 0);
    context.beginPath();
    context.arc(0, 0, 14, 0, Math.PI * 2);
    context.fillStyle = `hsl(${(index * 60 + seconds * 40) % 360} 90% 62%)`;
    context.fill();
    context.restore();
  }

  // `lighter` adds rather than covers, which is what makes the core bloom.
  context.save();
  context.globalCompositeOperation = "lighter";
  context.drawImage(badge, width / 2 - 32, height / 2 - 12, 64, 64);
  context.restore();

  // Text, at all three horizontal anchors, measured before it is drawn.
  context.font = "600 20px system-ui, sans-serif";
  context.fillStyle = "#e8ecff";
  context.textBaseline = "alphabetic";
  for (const [align, x] of [["left", 24], ["center", width / 2], ["right", width - 24]]) {
    context.textAlign = align;
    context.fillText(align, x, height - 24);
  }
  context.textAlign = "center";
  context.strokeStyle = "#ffd166";
  context.lineWidth = 1;
  context.strokeText("strokeText", width / 2, 48);

  const metrics = context.measureText("strokeText");
  const pixel = context.getImageData(width >> 1, height >> 1, 1, 1).data;
  readout.textContent = `${width}x${height} store · measured ${metrics.width.toFixed(1)}px `
    + `· centre rgba(${Array.from(pixel).join(", ")})`;

  // One encoded frame, put back into the document as an ordinary image. The
  // readback path end to end: rasterise the recorded scene, encode it, and hand
  // the bytes to the same subresource loader an `<img src>` uses. Taken from a
  // frame callback rather than a timer so a headless replay records it too.
  if (frames === 8) document.getElementById("shot").src = canvas.toDataURL();
  frames++;

  requestAnimationFrame(draw);
}

let frames = 0;
requestAnimationFrame(draw);
