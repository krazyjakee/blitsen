/** The private UI-test protocol. Each native call returns before Bun awaits I/O. */
export async function applicationTestLoop(native, entrypoint, identity, width, height) {
  const { createInterface } = await import("node:readline");
  const respond = value => process.stdout.write(`BLITSEN_TEST:${JSON.stringify(value)}\n`);
  const ready = JSON.parse(native.startApplicationTest(entrypoint, identity, width, height));
  const settle = async milliseconds => {
    if (!Number.isInteger(milliseconds) || milliseconds < 0 || milliseconds > 30000)
      throw new RangeError("settle is limited to 30000 ms per call");
    const deadline = performance.now() + milliseconds;
    do {
      native.tickApplicationTest();
      await Bun.sleep(4);
    } while (performance.now() < deadline);
  };
  const lines = createInterface({ input: process.stdin });
  let timer;
  try {
    await settle(50);
    respond(ready);
    timer = setInterval(() => native.tickApplicationTest(), 8);
    for await (const line of lines) {
      let request;
      try {
        request = JSON.parse(line);
        let result;
        if (request.command === "settle") {
          await settle(request.ms ?? 50);
          result = null;
        } else result = JSON.parse(native.applicationTestCommand(line));
        respond({ id: request.id, result });
        if (request.command === "close") break;
      } catch (error) {
        respond({ id: request?.id ?? null, error: String(error.message ?? error) });
      }
    }
  } finally {
    clearInterval(timer);
    lines.close();
    native.closeApplicationTest();
  }
}
