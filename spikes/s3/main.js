const native = require("./s3_window.node");
const frames = Number(process.env.S3_FRAMES ?? 120);

native.openWindow(frames);
const timer = setInterval(() => {
  if (native.pumpWindow()) {
    clearInterval(timer);
    console.log(native.windowStats());
  }
}, 16);

setTimeout(() => {
  clearInterval(timer);
  throw new Error("S3 window benchmark timed out");
}, frames * 25 + 15_000).unref();
