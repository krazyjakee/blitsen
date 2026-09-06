import { runExample } from "./build-addon.mjs";

await runExample({
  purpose: "hardware demo", example: "hardware", width: 1180, height: 820, title: "Blitsen Hardware",
  hints: [
    "blitsen/os: the processor, memory, storage and OS identity of this machine,",
    "none of which the web platform can ask for.",
    "Expect four tabs. Processor names the real CPU and animates one meter per",
    "thread once a second; Memory and Storage report real capacity; System names",
    "the kernel and the boot time. The first processor reading is '—' by design:",
    "it measures since boot rather than an interval, so it is discarded.",
  ],
});
