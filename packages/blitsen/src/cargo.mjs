import { execFile } from "node:child_process";
import { join } from "node:path";
import { promisify } from "node:util";

const execute = promisify(execFile);
async function runCargo([command, ...args]) {
  try { return { code: 0, ...await execute(command, args) }; }
  catch (error) { return { code: error.code, stdout: error.stdout ?? "", stderr: error.stderr ?? error.message }; }
}

/** Ask Cargo where it writes artifacts, including configured target directories. */
export async function cargoTargetDirectory(root, run = runCargo) {
  const result = await run(["cargo", "metadata", "--no-deps", "--format-version", "1",
    "--manifest-path", join(root, "Cargo.toml")], { capture: true });
  if (result.code !== 0) throw new Error(`cargo metadata exited ${result.code} for ${root}: ${result.stderr.trim()}`);
  const directory = JSON.parse(result.stdout).target_directory;
  if (typeof directory !== "string" || directory === "") throw new Error("cargo metadata named no target_directory");
  return directory;
}
