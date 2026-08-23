import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { measurementStorageEnvironment } from "./measure-export.mjs";

describe("measurement storage environment", () => {
  test("uses an absolute isolated data root on every runner family", () => {
    const root = join(process.cwd(), "temporary-measurement");
    expect(measurementStorageEnvironment("darwin", root)).toEqual({
      HOME: join(root, "home"),
    });
    expect(measurementStorageEnvironment("linux", root)).toEqual({
      XDG_DATA_HOME: join(root, "data"),
    });
    expect(measurementStorageEnvironment("win32", root)).toEqual({
      APPDATA: join(root, "app-data"),
      LOCALAPPDATA: join(root, "local-data"),
    });
  });
});
