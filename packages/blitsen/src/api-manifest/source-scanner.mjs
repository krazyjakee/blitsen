import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { NATIVE } from "./catalogue.mjs";

// Paths rather than URL objects: the runtime replaces URL in the CLI realm.
const RUNTIME_SOURCE_ROOT = "../../../../crates/blitsen-host/src/";
const RUNTIME_SOURCE = join(import.meta.dirname, `${RUNTIME_SOURCE_ROOT}dom_bridge.rs`);
export const SOURCE_NAME = "crates/blitsen-host/src/dom_bridge.rs";

// Everything below reads the bootstrap as the JavaScript it is, rather than a
// description of it kept alongside.
//
// The script is spliced together from `dom_bridge/bootstrap/*.js`, and the
// splice order lives in the Rust that evaluates it. Reading the order from
// there rather than restating it keeps the manifest describing the same script
// the runtime actually runs, and turns a renamed fragment into a loud failure.
export async function readBootstrapScript() {
  const rust = await readFile(RUNTIME_SOURCE, "utf8");
  const fragments = [...rust.matchAll(/include_str!\("(dom_bridge\/bootstrap\/[^"]+)"\)/g)]
    .map(([, path]) => join(import.meta.dirname, RUNTIME_SOURCE_ROOT + path));
  if (fragments.length === 0)
    throw new Error(`${SOURCE_NAME} no longer splices a bootstrap script`);
  return (await Promise.all(fragments.map(file => readFile(file, "utf8")))).join("");
}

// Blanks comments and literal contents while preserving every offset, so a
// structural walk cannot be confused by an apostrophe in a comment.
function blanked(script) {
  const characters = [...script];
  const previous = index => {
    for (let scan = index - 1; scan >= 0; scan--) if (!/\s/.test(script[scan])) return script[scan];
    return "";
  };
  let state = null;
  for (let index = 0; index < characters.length; index++) {
    const character = script[index];
    if (state === null) {
      if (character === "/" && script[index + 1] === "/") {
        characters[index] = characters[index + 1] = " ";
        index++;
        state = "line";
      } else if (character === "/" && script[index + 1] === "*") {
        characters[index] = characters[index + 1] = " ";
        index++;
        state = "block";
      } else if (character === "/" && !/[\w$)\]]/.test(previous(index))) {
        characters[index] = " ";
        state = "regex";
      } else if (character === '"' || character === "'" || character === "`") {
        characters[index] = " ";
        state = character;
      }
      continue;
    }
    if (state === "line") {
      if (character === "\n") state = null;
      else characters[index] = " ";
      continue;
    }
    if (state === "block") {
      if (character === "*" && script[index + 1] === "/") {
        characters[index] = characters[index + 1] = " ";
        index++;
        state = null;
      } else if (character !== "\n") characters[index] = " ";
      continue;
    }
    if (character === "\\") {
      characters[index] = characters[index + 1] = " ";
      index++;
      continue;
    }
    if (character === (state === "regex" ? "/" : state)) {
      characters[index] = " ";
      state = null;
    }
    else characters[index] = " ";
  }
  return characters.join("");
}

const identifierAt = (script, index) => /^[A-Za-z_$][\w$]*/.exec(script.slice(index))?.[0] ?? null;
const afterSpace = (script, index) => {
  while (index < script.length && /\s/.test(script[index])) index++;
  return index;
};
const sourceLine = (script, index) => script.slice(0, index).split("\n").length;

function matchingDelimiter(script, opening, open, close, context) {
  let depth = 0;
  for (let index = opening; index < script.length; index++) {
    if (script[index] === open) depth++;
    else if (script[index] === close && --depth === 0) return index;
  }
  throw new Error(`${context} has no closing ${close}`);
}

function classMemberKey(script, index, className) {
  let generator = false;
  if (script[index] === "*") {
    generator = true;
    index = afterSpace(script, index + 1);
  }
  if (script[index] === "[") {
    const end = matchingDelimiter(script, index, "[", "]", `${className}'s computed member`);
    return { name: null, index: afterSpace(script, end + 1) };
  }
  let name = identifierAt(script, index);
  if (!name)
    throw new Error(`${className} has an unsupported member at line ${sourceLine(script, index)}`);
  index = afterSpace(script, index + name.length);

  // These are modifiers only when another property key follows. `get()`,
  // `set()` and `static()` remain ordinary methods with those names.
  if (!generator && ["static", "async", "get", "set"].includes(name)
      && script[index] !== "(" && script[index] !== "=" && script[index] !== ";") {
    if (name === "static" && script[index] === "{") {
      const end = matchingDelimiter(script, index, "{", "}", `${className}'s static block`);
      return { name: null, index: afterSpace(script, end + 1), complete: true };
    }
    return classMemberKey(script, index, className);
  }
  return { name, index };
}

function classMembers(script, opening, closing, className) {
  const members = new Set();
  let index = opening + 1;
  while ((index = afterSpace(script, index)) < closing) {
    if (script[index] === ";") { index++; continue; }
    const key = classMemberKey(script, index, className);
    index = key.index;
    if (key.complete) continue;
    if (script[index] === "(") {
      const parameters = matchingDelimiter(script, index, "(", ")", `${className}.${key.name ?? "[computed]"}`);
      index = afterSpace(script, parameters + 1);
      if (script[index] !== "{")
        throw new Error(`${className}.${key.name ?? "[computed]"} must have a method body`);
      index = matchingDelimiter(script, index, "{", "}",
        `${className}.${key.name ?? "[computed]"}`) + 1;
    } else if (script[index] === "=") {
      // Bootstrap classes currently use methods and accessors. Supporting a
      // field is cheap, but its initializer must still be structurally closed
      // rather than letting the next declaration be mistaken for a member.
      let braces = 0, brackets = 0, parentheses = 0;
      for (index++; index < closing; index++) {
        if (script[index] === "{") braces++;
        else if (script[index] === "}") { if (braces === 0) break; braces--; }
        else if (script[index] === "[") brackets++;
        else if (script[index] === "]") brackets--;
        else if (script[index] === "(") parentheses++;
        else if (script[index] === ")") parentheses--;
        else if (script[index] === ";" && braces === 0 && brackets === 0 && parentheses === 0) {
          index++;
          break;
        }
      }
    } else if (script[index] === ";") index++;
    else throw new Error(`${className}.${key.name ?? "[computed]"} has an unsupported declaration`);
    if (key.name && key.name !== "constructor") members.add(key.name);
  }
  return members;
}

function runtimeClassesAndInstances(script) {
  const classes = new Map();
  const instanceDeclarations = [];
  let braces = 0, brackets = 0, parentheses = 0;
  for (let index = 0; index < script.length;) {
    const identifier = identifierAt(script, index);
    if (identifier && braces === 0 && brackets === 0 && parentheses === 0) {
      if (identifier === "class") {
        let cursor = afterSpace(script, index + identifier.length);
        const name = identifierAt(script, cursor);
        if (!name) throw new Error(`the bootstrap has an unnamed class at line ${sourceLine(script, index)}`);
        cursor = afterSpace(script, cursor + name.length);
        let base;
        if (identifierAt(script, cursor) === "extends") {
          cursor = afterSpace(script, cursor + "extends".length);
          base = identifierAt(script, cursor);
          if (!base) throw new Error(`${name} has an unsupported extends declaration`);
          cursor = afterSpace(script, cursor + base.length);
        }
        if (script[cursor] !== "{") throw new Error(`${name} has no class body`);
        const closing = matchingDelimiter(script, cursor, "{", "}", `class ${name}`);
        if (classes.has(name)) throw new Error(`the bootstrap declares class ${name} twice`);
        classes.set(name, { base, members: classMembers(script, cursor, closing, name) });
        index = closing + 1;
        continue;
      }
      if (identifier === "const") {
        const declaration = /^const\s+([A-Za-z_$][\w$]*)\s*=\s*(?:new\s+([A-Za-z_$][\w$]*)\s*\(\s*\)|Object\s*\.\s*create\s*\(\s*([A-Za-z_$][\w$]*)\s*\.\s*prototype\s*\))\s*;/
          .exec(script.slice(index));
        if (declaration)
          instanceDeclarations.push([declaration[1], declaration[2] ?? declaration[3]]);
      }
      index += identifier.length;
      continue;
    }
    if (script[index] === "{") braces++;
    else if (script[index] === "}") braces--;
    else if (script[index] === "[") brackets++;
    else if (script[index] === "]") brackets--;
    else if (script[index] === "(") parentheses++;
    else if (script[index] === ")") parentheses--;
    index++;
  }
  if (braces !== 0 || brackets !== 0 || parentheses !== 0)
    throw new Error("the bootstrap has unbalanced delimiters");
  const instances = new Map(instanceDeclarations.filter(([, className]) => classes.has(className)));
  return { classes, instances };
}

function objectKeys(script, declaration) {
  const start = script.indexOf(declaration);
  if (start < 0) throw new Error(`the bootstrap no longer declares ${declaration}`);
  const keys = [];
  let depth = 0;
  let expectKey = false;
  for (let index = start + declaration.length - 1; index < script.length; index++) {
    const character = script[index];
    if ("{[(".includes(character)) {
      depth++;
      expectKey = depth === 1;
    } else if ("}])".includes(character)) {
      if (--depth === 0) return keys;
    } else if (character === "," && depth === 1) expectKey = true;
    else if (expectKey && /[A-Za-z_$]/.test(character)) {
      const key = /^[\w$]+/.exec(script.slice(index))[0];
      keys.push(key);
      expectKey = false;
      index += key.length - 1;
    }
  }
  throw new Error(`${declaration} is not a closed object literal`);
}

function stringList(script, pattern, name) {
  const match = pattern.exec(script);
  if (!match) throw new Error(`the bootstrap no longer installs ${name} the way this reader parses`);
  return [...match[1].matchAll(/"([^"]+)"/g)].map(([, value]) => value);
}

// Reads the globals, class members and deliberate deletions out of the bootstrap.
//
// Line endings are normalised first: every pattern below is anchored on `\n`
// against source this reads as bytes, and a Windows checkout with
// `core.autocrlf` on hands it CRLF. The repository pins `eol=lf` in
// `.gitattributes` so that does not happen, and this is the second lock on the
// same door — the first release dry run failed here, reporting that the
// bootstrap had stopped installing globals it installs perfectly well (#134).
export function extractRuntimeSurface(source) {
  const script = source.includes("\r\n") ? source.replaceAll("\r\n", "\n") : source;
  const structure = blanked(script);
  const globals = new Set(objectKeys(structure, "const globals = {"));
  for (const [, name] of structure.matchAll(/globalThis\.([A-Za-z_$][\w$]*)\s*=[^=]/g))
    globals.add(name);
  for (const name of stringList(script,
    /for \(const method of (\[[^\]]*\])\)\n\s*Object\.defineProperty\(globalThis, method,/,
    "the global EventTarget methods")) globals.add(name);
  for (const name of stringList(script,
    /for \(const \[name, value\] of (\[\[[\s\S]*?\]\])\)\n\s*Object\.defineProperty\(globalThis, name,/,
    "location and history")) globals.add(name);
  // The document's scroll offsets, which are accessors rather than values and
  // so are not in the `globals` object literal. Only the first name of each
  // pair is a global; the second is the element property it reads.
  for (const [index, name] of stringList(script,
    /for \(const \[name, axis\] of (\[\[[\s\S]*?\]\])\)\n\s*Object\.defineProperty\(globalThis, name,/,
    "the scroll offsets").entries()) if (index % 2 === 0) globals.add(name);
  const deleted = new Set(stringList(script,
    /for \(const key of (\[[\s\S]*?\])\) \{\n\s*try \{ delete globalThis\[key\]; \} catch \{\}/,
    "the deliberately absent globals"));
  // A global the bootstrap builds only if the host gave it what it needs, and
  // withdraws by name when it did not. Read from the structure rather than
  // declared beside CONDITIONAL, so a claim that an API is conditional stands on
  // the runtime being able to drop it.
  const conditional = [
    ...[...structure.matchAll(
      /\n\s*if \(![A-Za-z_$][\w$]*\) try \{ delete globalThis\.([A-Za-z_$][\w$]*); \} catch \{\}/g)]
      .map(([, name]) => name),
    ...[...structure.matchAll(
      /\n\s*if \(![A-Za-z_$][\w$]*\) try \{ delete ([A-Za-z_$][\w$]*)\.prototype\.([A-Za-z_$][\w$]*); \} catch \{\}/g)]
      .map(([, owner, member]) => `${owner}.${member}`),
  ];

  const { classes, instances } = runtimeClassesAndInstances(structure);
  const native = new Map(Object.keys(NATIVE).map(module =>
    [module, new Set(objectKeys(structure, `const native${capitalized(module)} = {`))]));
  return { globals: [...globals].filter(name => !name.startsWith("__blitsen")), classes, instances,
    deleted: [...deleted], conditional, native };
}

const capitalized = name => `${name[0].toUpperCase()}${name.slice(1)}`;
