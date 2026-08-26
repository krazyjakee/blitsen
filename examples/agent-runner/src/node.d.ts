declare module "node:fs/promises" {
  export function readFile(path: string, encoding: "utf8"): Promise<string>;
  export function readdir(path: string, options: { withFileTypes: true }): Promise<Array<{
    name: string;
    isDirectory(): boolean;
    isFile(): boolean;
  }>>;
  export function stat(path: string): Promise<{ mtimeMs: number }>;
}

declare module "node:child_process" {
  interface Stream { on(event: "data", callback: (data: Uint8Array | string) => void): void; }
  export interface Child {
    stdout: Stream | null;
    stderr: Stream | null;
    on(event: "spawn", callback: () => void): void;
    on(event: "error", callback: (error: Error) => void): void;
    on(event: "close", callback: (code: number | null, signal: string | null) => void): void;
    kill(signal?: string): boolean;
    unref(): void;
  }
  export function spawn(command: string, args: string[], options: {
    cwd: string;
    env: Record<string, string | undefined>;
    stdio: ["ignore", "pipe", "pipe"] | "ignore";
    detached?: boolean;
  }): Child;
}

declare const process: { env: Record<string, string | undefined> };
