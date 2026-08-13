import { createHash } from "node:crypto";
import { chmod, readFile, stat, writeFile } from "node:fs/promises";

// The Phase 2 link step (issue #88): the application is appended to Blitsen's
// own runtime executable and read back out of it at startup, without ever being
// unpacked. The format is specified in `crates/blitsen-core/src/bundle.rs`, and
// that file is the reader. This is the writer, and it is here rather than in
// Rust because a cross-target export links an executable it cannot run —
// `cli-bundle.test.mjs` holds the two implementations to the same bytes.
const PAYLOAD_MAGIC = Buffer.from("BLITSEN\0", "latin1");
const TRAILER_MAGIC = Buffer.from("BLITSEN\x1a", "latin1");
const HEADER_SIZE = 32;
const TRAILER_SIZE = 64;
export const FORMAT_VERSION = 1;

// A bundle path addresses a file inside the application and nothing else. The
// reader refuses anything else when it reads the index, so refusing it here
// turns a bad path into a build error rather than a runtime one.
function checkPath(path) {
  const bad = !path || path.startsWith("/") || path.includes("\\") || path.includes("\0")
    || path.split("/").some(segment => !segment || segment === "." || segment === ".."
      || segment.endsWith(":"));
  if (bad) throw new Error(`cannot bundle ${JSON.stringify(path)}: not a path inside the application`);
  return path;
}

/**
 * Serializes the payload: a version header, an index, then the file data.
 *
 * `files` is a map of application-relative path to contents. Entries are sorted
 * so the same input links to the same bytes, which is what lets a build be
 * compared with the one before it.
 */
export function buildPayload(files) {
  const entries = [...files.entries()]
    .map(([path, bytes]) => [checkPath(path), Buffer.from(bytes)])
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0));

  const index = [];
  const data = [];
  let offset = 0;
  for (const [path, bytes] of entries) {
    const name = Buffer.from(path, "utf8");
    const entry = Buffer.alloc(20);
    entry.writeUInt32LE(name.length, 0);
    entry.writeBigUInt64LE(BigInt(offset), 4);
    entry.writeBigUInt64LE(BigInt(bytes.length), 12);
    index.push(entry, name);
    data.push(bytes);
    offset += bytes.length;
  }
  const indexBytes = Buffer.concat(index);
  const dataBytes = Buffer.concat(data);

  const header = Buffer.alloc(HEADER_SIZE);
  PAYLOAD_MAGIC.copy(header, 0);
  header.writeUInt32LE(FORMAT_VERSION, 8);
  header.writeUInt32LE(0, 12);
  header.writeUInt32LE(entries.length, 16);
  header.writeUInt32LE(indexBytes.length, 20);
  header.writeBigUInt64LE(BigInt(dataBytes.length), 24);
  return Buffer.concat([header, indexBytes, dataBytes]);
}

/** Serializes the trailer that locates and checksums a payload. */
export function buildTrailer(payload, payloadOffset) {
  const trailer = Buffer.alloc(TRAILER_SIZE);
  createHash("sha256").update(payload).digest().copy(trailer, 0);
  trailer.writeBigUInt64LE(BigInt(payloadOffset), 32);
  trailer.writeBigUInt64LE(BigInt(payload.length), 40);
  trailer.writeUInt32LE(FORMAT_VERSION, 48);
  trailer.writeUInt32LE(0, 52);
  TRAILER_MAGIC.copy(trailer, 56);
  return trailer;
}

/**
 * Links `files` into a copy of the runtime executable at `runtime`.
 *
 * Signing happens afterwards, never before: appending to an already-signed
 * binary is what breaks the signature (see the Rust module's "Signing" note),
 * so the export pipeline's signing hook runs over the result of this.
 *
 * Returns what was written, for the `④ link` line the CLI prints.
 */
export async function linkBundle({ runtime, output, files }) {
  const executable = await readFile(runtime);
  const payload = buildPayload(files);
  const trailer = buildTrailer(payload, executable.length);
  await writeFile(output, Buffer.concat([executable, payload, trailer]));
  // The runtime arrives from an npm tarball, which does not always preserve the
  // executable bit; the linked application is useless without it.
  const mode = (await stat(runtime)).mode & 0o777;
  await chmod(output, mode | 0o111);
  return {
    files: files.size,
    payloadBytes: payload.length,
    totalBytes: executable.length + payload.length + trailer.length,
    digest: trailer.subarray(0, 32).toString("hex"),
  };
}

/**
 * Reads a linked executable back, for tests and `blitsen doctor`.
 *
 * A deliberately independent decoder rather than a call into the runtime: what
 * it is checking is that the bytes on disk say what the CLI meant them to, and
 * asking the writer to confirm its own output would check nothing.
 */
export function readBundle(executable) {
  const trailerAt = findTrailer(executable);
  if (trailerAt === null) return null;
  const trailer = executable.subarray(trailerAt, trailerAt + TRAILER_SIZE);
  const version = trailer.readUInt32LE(48);
  const offset = Number(trailer.readBigUInt64LE(32));
  const length = Number(trailer.readBigUInt64LE(40));
  const payload = executable.subarray(offset, offset + length);
  const digest = trailer.subarray(0, 32).toString("hex");
  const actual = createHash("sha256").update(payload).digest("hex");

  const count = payload.readUInt32LE(16);
  const indexLength = payload.readUInt32LE(20);
  const index = payload.subarray(HEADER_SIZE, HEADER_SIZE + indexLength);
  const dataAt = HEADER_SIZE + indexLength;
  const files = new Map();
  let cursor = 0;
  for (let entry = 0; entry < count; entry += 1) {
    const nameLength = index.readUInt32LE(cursor);
    const at = Number(index.readBigUInt64LE(cursor + 4));
    const size = Number(index.readBigUInt64LE(cursor + 12));
    const path = index.subarray(cursor + 20, cursor + 20 + nameLength).toString("utf8");
    cursor += 20 + nameLength;
    files.set(path, payload.subarray(dataAt + at, dataAt + at + size));
  }
  return { version, offset, length, digest, verified: digest === actual, files };
}

// The magic alone is not proof: the runtime carries a copy of it in its own
// read-only data. A candidate is only taken when the payload it points at
// actually starts with the payload magic, which is what the Rust reader does.
function findTrailer(executable) {
  for (let at = executable.length - TRAILER_SIZE; at >= 0; at -= 1) {
    if (!executable.subarray(at + 56, at + 64).equals(TRAILER_MAGIC)) continue;
    const offset = Number(executable.readBigUInt64LE(at + 32));
    const length = Number(executable.readBigUInt64LE(at + 40));
    if (offset < 0 || length < HEADER_SIZE || offset + length > at) continue;
    if (!executable.subarray(offset, offset + 8).equals(PAYLOAD_MAGIC)) continue;
    return at;
  }
  return null;
}
