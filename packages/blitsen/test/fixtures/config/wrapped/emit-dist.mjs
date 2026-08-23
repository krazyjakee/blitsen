// Stands in for the user's toolchain: Blitsen runs this command and consumes the
// directory it writes, without knowing anything about it.
import { mkdir, writeFile } from "node:fs/promises";

await mkdir("dist", { recursive: true });
await writeFile("dist/index.html", "<!doctype html>\n<title>Wrapped</title>\n<p>wrapped</p>\n");
await mkdir("native", { recursive: true });
await writeFile("native/tray.png", Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgQIAffRr7QAAAABJRU5ErkJggg==",
  "base64",
));
