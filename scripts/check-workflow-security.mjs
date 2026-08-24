import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const ACTION_PIN = /^[^/\s]+\/[^@\s]+@[0-9a-f]{40}$/;
const UNTRUSTED_EXPRESSION =
  /\$\{\{\s*(?:inputs(?:\.|\[)|github\.(?:event(?:\.|\[)|head_ref\b|base_ref\b|ref_name\b))/;

function visit(value, path, errors) {
  if (Array.isArray(value)) {
    value.forEach((item, index) => visit(item, `${path}[${index}]`, errors));
    return;
  }
  if (value === null || typeof value !== "object") return;

  for (const [key, child] of Object.entries(value)) {
    const childPath = path ? `${path}.${key}` : key;
    if (
      key === "uses" &&
      typeof child === "string" &&
      !child.startsWith("./") &&
      !ACTION_PIN.test(child)
    ) {
      errors.push(`${childPath}: external action is not pinned to a full commit SHA`);
    }
    if (
      key === "run" &&
      typeof child === "string" &&
      UNTRUSTED_EXPRESSION.test(child)
    ) {
      errors.push(
        `${childPath}: pass untrusted expressions through env instead of interpolating them in run`,
      );
    }
    visit(child, childPath, errors);
  }
}

export function checkWorkflowSource(source, file = "workflow.yml") {
  const errors = [];
  let workflow;
  try {
    workflow = Bun.YAML.parse(source);
  } catch (error) {
    return [`${file}: invalid YAML: ${error.message}`];
  }

  visit(workflow, file, errors);

  source.split("\n").forEach((line, index) => {
    const match = line.match(/^\s*(?:-\s*)?uses:\s*([^\s#]+)\s*(?:#\s*(\S.*))?$/);
    if (!match || match[1].startsWith("./")) return;
    const [, , versionComment] = match;
    if (!versionComment) {
      errors.push(`${file}:${index + 1}: pinned action is missing a readable version comment`);
    }
  });

  return errors;
}

export async function checkWorkflowDirectory(directory = ".github/workflows") {
  const names = (await readdir(directory))
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .sort();
  const errors = [];
  for (const name of names) {
    const file = join(directory, name);
    errors.push(...checkWorkflowSource(await readFile(file, "utf8"), file));
  }
  return { errors, files: names.length };
}

if (import.meta.main) {
  const { errors, files } = await checkWorkflowDirectory();
  if (errors.length > 0) {
    errors.forEach((error) => console.error(`::error::${error}`));
    process.exitCode = 1;
  } else {
    console.log(`Workflow security policy passed for ${files} workflows`);
  }
}
