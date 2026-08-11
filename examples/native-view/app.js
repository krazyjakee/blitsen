const view = document.getElementById("view");
const size = document.getElementById("size");
const surface = view.acquireSurface();

let pixels = null;
let generation = -1;

view.addEventListener("resize", () => { pixels = null; });

function frame(timestamp) {
  if (pixels === null || generation !== surface.generation) {
    generation = surface.generation;
    pixels = new Uint8Array(surface.byteLength);
    size.textContent = `${surface.width}x${surface.height} @${surface.devicePixelRatio}x`;
  }
  const width = surface.width;
  const height = surface.height;
  const phase = timestamp / 900;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      pixels[i] = (x * 255 / width) | 0;
      pixels[i + 1] = (y * 255 / height) | 0;
      pixels[i + 2] = ((Math.sin(phase + (x + y) / 60) * 0.5 + 0.5) * 255) | 0;
      pixels[i + 3] = 255;
    }
  }
  surface.write(pixels);
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
