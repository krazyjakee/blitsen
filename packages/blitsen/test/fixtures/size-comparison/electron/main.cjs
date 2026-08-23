const { app, BrowserWindow } = require("electron");
const { join } = require("node:path");

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 800, height: 600, show: true });
  window.loadFile(join(__dirname, "index.html"));
});

app.on("window-all-closed", () => app.quit());
