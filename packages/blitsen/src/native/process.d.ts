// `blitsen/process`: managed child processes for CLI-driven applications.
//
// An executable and an argument array, never a command line. Output arrives as
// byte chunks in the order it was read, the exit after the last of them, and
// every event on a frame turn. `kill` ends the child's whole process tree.
import type { NativeNamespace, NativeProcess } from "./native.js";

export type {
  ChildProcess, ExitStatus, NativeProcess, SpawnOptions, StdioMode,
} from "./native.js";

declare const process: NativeNamespace<NativeProcess>;
export default process;
