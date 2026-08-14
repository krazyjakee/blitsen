// `blitsen/os`: what machine this is — processor, memory, storage, OS identity.
//
// The displays are deliberately not here. They are `window.monitors()`, which
// already reports each monitor's size, position and scale factor; a second list
// on this module could disagree with that one.
//
// Every member is a reading rather than a constant, so a monitor calls them on a
// timer. `cpu().usage` measures the interval since the previous call, which
// makes the first call the odd one out: it reports a baseline against the
// counters' own origin — on Linux, the average since boot — so discard it and
// start from the second.
import type { NativeNamespace, NativeOs } from "./native.js";

export type { Cpu, CpuCore, Host, Memory, NativeOs, Volume } from "./native.js";

declare const os: NativeNamespace<NativeOs>;
export default os;
