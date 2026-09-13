export type Locator = string | { selector?: string; role?: string; name?: string; text?: string };
export type Assertion = string | (() => unknown);
export interface PointerInput {
  x: number; y: number; button?: number; deltaX?: number; deltaY?: number;
  ctrlKey?: boolean; shiftKey?: boolean; altKey?: boolean; metaKey?: boolean;
}
export interface ElementSnapshot {
  tag: string; id: string; handle: string; text: string; role: string | null; name: string;
  box: { x: number; y: number; width: number; height: number };
  devicePixelRatio: number; viewport: { width: number; height: number };
  disabled: boolean; value: string | null; checked: boolean | null; focused: boolean;
  scrollLeft: number; scrollTop: number;
}
export interface TestApplication {
  viewport: { width: number; height: number; devicePixelRatio: number };
  query(locator: Locator): Promise<ElementSnapshot[]>;
  evaluate(expression: Assertion): Promise<unknown>;
  assert(expression: Assertion): Promise<void>;
  waitFor(expression: Assertion, options?: { timeout?: number }): Promise<void>;
  /** Advances timers, microtasks, native work, animation frames and layout for this duration. */
  settle(ms?: number): Promise<void>;
  click(locator: Locator | PointerInput): Promise<void>;
  pointer(type: "pointermove" | "pointerdown" | "pointerup" | "pointercancel", input: PointerInput): Promise<void>;
  wheel(input: PointerInput): Promise<void>;
  key(type: "keydown" | "keyup", input: { key: string; code?: string; repeat?: boolean;
    ctrlKey?: boolean; shiftKey?: boolean; altKey?: boolean; metaKey?: boolean }): Promise<void>;
  /** Commits text through the native IME path into the focused control. */
  type(text: string): Promise<void>;
  screenshot(path?: string): Promise<Uint8Array>;
  eventLog(): Promise<Array<{ type: string; target: { tag: string; id: string; handle: string } | null;
    defaultPrevented: boolean; clientX?: number; clientY?: number }>>;
  close(): Promise<void>;
}
export interface LaunchOptions {
  env?: Record<string, string | undefined>; width?: number; height?: number;
  args?: string[]; cwd?: string; timeout?: number; artifactsDir?: string;
  screenshots?: "failures" | "steps"; onLog?: (line: string) => void;
}
export function launch(exportPath: string, options?: LaunchOptions): Promise<TestApplication>;
declare const test: { launch: typeof launch };
export default test;
