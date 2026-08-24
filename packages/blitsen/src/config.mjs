import { spawn } from "node:child_process";
import { readFile, realpath } from "node:fs/promises";
import { delimiter, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { pngDimensions } from "./packaging.mjs";

const trayString = { type: "string", minLength: 1 };
const trayEnabled = { type: "boolean" };
const trayActionProperties = {
  type: { const: "action" }, label: trayString, enabled: trayEnabled,
  accelerator: trayString, icon: { ...trayString, pattern: "\\.png$" },
};
// The item shapes both menu surfaces accept verbatim: the host parses one
// model for the tray and the application menu, so the separator, checkbox and
// radio variants are declared once and referenced from both oneOf lists.
const separatorMenuItem = {
  type: "object", additionalProperties: false, required: ["type"],
  properties: { type: { const: "separator" } },
};
const checkboxMenuItem = {
  type: "object", additionalProperties: false, required: ["type", "id", "label"],
  properties: {
    type: { const: "checkbox" }, id: trayString, label: trayString,
    enabled: trayEnabled, checked: { type: "boolean" }, accelerator: trayString,
  },
};
const radioMenuItem = {
  type: "object", additionalProperties: false,
  required: ["type", "id", "label", "group"],
  properties: {
    type: { const: "radio" }, id: trayString, label: trayString, group: trayString,
    enabled: trayEnabled, checked: { type: "boolean" }, accelerator: trayString,
  },
};
const trayMenuItem = {
  oneOf: [
    {
      type: "object", additionalProperties: false, required: ["action"],
      properties: {
        ...trayActionProperties,
        action: { type: "string", enum: ["show", "hide", "quit"] },
      },
    },
    {
      type: "object", additionalProperties: false, required: ["id", "label"],
      properties: { ...trayActionProperties, id: trayString },
    },
    {
      type: "object", additionalProperties: false, required: ["action"],
      properties: {
        action: { const: "separator" }, label: trayString, enabled: trayEnabled,
      },
    },
    separatorMenuItem,
    checkboxMenuItem,
    radioMenuItem,
    {
      type: "object", additionalProperties: false, required: ["type", "label", "menu"],
      properties: {
        type: { const: "submenu" }, label: trayString, enabled: trayEnabled,
        icon: { ...trayString, pattern: "\\.png$" },
        menu: { type: "array", items: { $ref: "#/definitions/trayMenuItem" } },
      },
    },
  ],
};

// The application menu's own item vocabulary. It shares the tray's nesting,
// checkable and radio shapes because the host parses one model for both, and
// differs where the two surfaces genuinely differ: roles instead of built-in
// tray actions, no icons, and a bar whose top level is submenus only.
const menuRoles = [
  "about", "services", "hide", "hideOthers", "showAll", "quit", "closeWindow",
  "minimize", "maximize", "fullscreen", "bringAllToFront",
  "undo", "redo", "cut", "copy", "paste", "selectAll",
];
const menuSubmenuRoles = ["application", "edit", "window", "help"];
const menuItem = {
  oneOf: [
    {
      type: "object", additionalProperties: false, required: ["type", "role"],
      properties: { type: { const: "role" }, role: { type: "string", enum: menuRoles } },
    },
    separatorMenuItem,
    {
      type: "object", additionalProperties: false, required: ["id", "label"],
      properties: {
        type: { const: "action" }, id: trayString, label: trayString,
        enabled: trayEnabled, accelerator: trayString,
      },
    },
    checkboxMenuItem,
    radioMenuItem,
    {
      type: "object", additionalProperties: false, required: ["type", "label", "menu"],
      properties: {
        type: { const: "submenu" }, label: trayString, enabled: trayEnabled,
        role: { type: "string", enum: menuSubmenuRoles },
        menu: { type: "array", items: { $ref: "#/definitions/menuItem" } },
      },
    },
  ],
};

// One canonical location: the "blitsen" key of package.json. This object is the
// contract, validated against below and published verbatim as
// config.schema.json for editor completion, so the two cannot drift.
export const CONFIG_SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  $id: "https://raw.githubusercontent.com/krazyjakee/blitsen/main/packages/blitsen/src/config.schema.json",
  title: "Blitsen configuration",
  description: "The \"blitsen\" key of package.json: build and static-output settings plus "
    + "native desktop presentation options.",
  type: "object",
  additionalProperties: false,
  required: ["output"],
  definitions: { trayMenuItem, menuItem },
  allOf: [
    {
      if: {
        required: ["window"],
        properties: { window: { required: ["type"], properties: { type: { const: "hidden" } } } },
      },
      then: { required: ["tray"] },
    },
    {
      if: {
        required: ["tray"],
        properties: {
          tray: { required: ["closeToTray"], properties: { closeToTray: { const: true } } },
        },
      },
      then: {
        properties: {
          tray: {
            required: ["contextMenu"],
            properties: {
              contextMenu: {
                description: "closeToTray is checked recursively by Blitsen's semantic validator.",
              },
            },
          },
        },
      },
    },
  ],
  properties: {
    build: {
      type: "string",
      minLength: 1,
      description: "Command run before ingest, from the directory holding this package.json. "
        + "Blitsen runs it and consumes its output directory; it never inspects or configures "
        + "the build tool.",
    },
    output: {
      type: "string",
      minLength: 1,
      description: "Directory of static web output, relative to this package.json. "
        + "It must contain index.html.",
    },
    name: {
      type: "string",
      minLength: 1,
      description: "Application name. Sets the native window title and the default output file name.",
    },
    addons: {
      type: "array",
      items: { type: "string", minLength: 1 },
      description: "Native .node addons carried into the export, relative to this package.json. "
        + "They live outside the output directory more often than not, so ingest cannot reach "
        + "them and they have to be declared.",
    },
    window: {
      type: "object",
      additionalProperties: false,
      description: "Native window creation options.",
      properties: {
        type: {
          type: "string",
          enum: ["normal", "borderless", "fullscreen", "hidden"],
          description: "Initial native window presentation. Defaults to normal.",
        },
        resizable: { type: "boolean", description: "Whether the user can resize the window." },
        transparent: {
          type: "boolean",
          description: "Request a transparent native surface. Support depends on the compositor.",
        },
        alwaysOnTop: {
          type: "boolean",
          description: "Request that the window stay above normal windows.",
        },
      },
    },
    tray: {
      type: "object",
      additionalProperties: false,
      required: ["icon"],
      description: "System tray icon and its built-in context-menu actions.",
      properties: {
        icon: {
          type: "string",
          minLength: 1,
          pattern: "\\.png$",
          description: "PNG tray icon, relative to this package.json.",
        },
        tooltip: { type: "string", minLength: 1 },
        openOnClick: {
          type: "boolean",
          description: "Show and focus the window when the tray icon is activated. Defaults to true.",
        },
        closeToTray: {
          type: "boolean",
          description: "Hide the window instead of exiting when its close control is used.",
        },
        contextMenu: {
          type: "array",
          items: { $ref: "#/definitions/trayMenuItem" },
          description: "Built-in or custom actions, separators, checkboxes, radio groups and nested submenus.",
        },
      },
    },
    menu: {
      type: "object",
      additionalProperties: false,
      required: ["menu"],
      description: "Application menu installed at startup: the macOS main menu and the Windows "
        + "window menu bar. Independent of the tray, and replaceable at run time through "
        + "blitsen/menu.",
      properties: {
        menu: {
          type: "array",
          items: { $ref: "#/definitions/menuItem" },
          description: "Top-level submenus, in the order they appear in the bar.",
        },
      },
    },
  },
};

function describeType(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "an array";
  return `a ${typeof value}`;
}

export function validateConfig(config, source) {
  const fail = (detail) => { throw new Error(`invalid blitsen config in ${source}: ${detail}`); };
  const known = Object.keys(CONFIG_SCHEMA.properties);
  if (typeof config !== "object" || config === null || Array.isArray(config)) {
    fail(`expected an object, found ${describeType(config)}`);
  }
  for (const key of Object.keys(config)) {
    if (!known.includes(key)) fail(`unknown key "${key}" (known keys: ${known.join(", ")})`);
  }
  for (const key of CONFIG_SCHEMA.required) {
    if (!(key in config)) fail(`missing required key "${key}"`);
  }
  const checkValue = (label, value, rule) => {
    // Recursive tray entries are validated by `validateTrayMenu` below. The
    // published schema retains the complete oneOf tree for editors and generic
    // JSON Schema validators.
    if (rule.$ref || rule.oneOf) return;
    if (rule.type === "array") {
      if (!Array.isArray(value)) fail(`${label} must be an array, found ${describeType(value)}`);
      value.forEach((item, index) => checkValue(`${label.slice(0, -1)}[${index}]"`, item, rule.items));
      return;
    }
    if (rule.type === "object") {
      if (typeof value !== "object" || value === null || Array.isArray(value)) {
        fail(`${label} must be an object, found ${describeType(value)}`);
      }
      const objectKeys = Object.keys(rule.properties);
      for (const key of Object.keys(value)) {
        if (!objectKeys.includes(key)) {
          fail(`${label.slice(0, -1)}.${key}" is unknown (known keys: ${objectKeys.join(", ")})`);
        }
      }
      for (const key of rule.required ?? []) {
        if (!(key in value)) fail(`${label} is missing required key "${key}"`);
      }
      for (const [key, childRule] of Object.entries(rule.properties)) {
        if (key in value) checkValue(`${label.slice(0, -1)}.${key}"`, value[key], childRule);
      }
      return;
    }
    if (typeof value !== rule.type) {
      fail(`${label} must be a ${rule.type}, found ${describeType(value)}`);
    }
    if (rule.minLength !== undefined && value.trim().length < rule.minLength) {
      fail(`${label} must not be empty`);
    }
    if (rule.enum && !rule.enum.includes(value)) {
      fail(`${label} must be one of ${rule.enum.join(", ")}, found ${JSON.stringify(value)}`);
    }
    if (rule.pattern && !(new RegExp(rule.pattern)).test(value)) {
      fail(`${label} must match ${rule.pattern}`);
    }
  };
  for (const [key, rule] of Object.entries(CONFIG_SCHEMA.properties)) {
    if (key in config) checkValue(`"${key}"`, config[key], rule);
  }
  if (config.window?.type === "hidden" && !config.tray) {
    fail('"window.type" hidden requires a "tray" configuration');
  }
  const hasQuit = config.tray?.contextMenu
    ? validateTrayMenu(config.tray.contextMenu, fail)
    : false;
  if (config.tray?.closeToTray && !hasQuit) {
    fail('"tray.closeToTray" requires a quit action in "tray.contextMenu"');
  }
  if (config.menu) validateApplicationMenu(config.menu.menu, fail);
  return config;
}

// The limits the host enforces on either menu surface. The failure sentences
// below spell the numbers out rather than interpolating these constants,
// because test/menu-parity.test.mjs reads the "at most N levels/entries"
// sentences from this source and holds them equal to the Rust parser's.
const MENU_MAX_DEPTH = 16;
const MENU_MAX_ITEMS = 512;
// The accelerator grammar: zero or more unique modifiers, then exactly one key.
const ACCELERATOR_MODIFIERS = new Set([
  "ctrl", "control", "alt", "option", "shift", "cmd", "command", "super", "meta",
  "cmdorctrl", "commandorcontrol",
]);

// Applies the semantics the recursive schema cannot state, as the host does.
// One walk serves the tray and the application menu: the shared frame is the
// depth and item limits, the object-shape guard, radio-group consecutiveness
// and the exactly-one-checked sweep, while `rules` carries what the surfaces
// genuinely do not share — item vocabularies, message wording and the menu
// bar's top-level-submenu-only rule.
function validateMenuTree(menu, fail, rules) {
  const ids = new Set();
  let count = 0;
  const nonEmpty = (value, description) => {
    if (typeof value !== "string" || value.trim().length === 0) fail(`${description} must not be empty`);
  };
  const keys = (item, allowed, description) => {
    for (const key of Object.keys(item)) {
      if (!allowed.includes(key)) fail(`${description}.${key} is not allowed`);
    }
  };
  const accelerator = (value, description) => {
    nonEmpty(value, `${description}.accelerator`);
    const parts = value.split("+").map(part => part.trim().toLowerCase());
    if (parts.some(part => !part)
      || ACCELERATOR_MODIFIERS.has(parts.at(-1))
      || parts.slice(0, -1).some(part => !ACCELERATOR_MODIFIERS.has(part))
      || new Set(parts.slice(0, -1)).size !== parts.length - 1) {
      fail(`${description}.accelerator must put unique modifiers before exactly one key`);
    }
  };
  const enabled = (item, description) => {
    if ("enabled" in item && typeof item.enabled !== "boolean") {
      fail(`${description}.enabled must be a boolean`);
    }
  };
  const uniqueId = id => {
    if (ids.has(id)) fail(rules.duplicateId);
    ids.add(id);
  };
  const level = (items, depth) => {
    if (!Array.isArray(items)) fail(rules.arrays);
    if (depth > MENU_MAX_DEPTH) fail(rules.depth);
    let activeRadio = null;
    const closedRadios = new Set();
    const checkedRadios = new Map();
    const tools = {
      nonEmpty, keys, accelerator, enabled, uniqueId,
      recurse: nested => level(nested, depth + 1),
      radio: (item, description) => {
        nonEmpty(item.group, `${description}.group`);
        checkedRadios.set(item.group, (checkedRadios.get(item.group) ?? 0) + Number(item.checked === true));
      },
    };
    for (const [index, item] of items.entries()) {
      const description = `${rules.subject} item ${index + 1} at depth ${depth}`;
      if (++count > MENU_MAX_ITEMS) fail(rules.count);
      if (typeof item !== "object" || item === null || Array.isArray(item)) {
        fail(`${description} must be an object`);
      }
      const type = rules.typeOf(item);
      rules.beforeItem?.(type, depth);
      const radio = type === "radio" ? item.group : null;
      if (radio !== activeRadio) {
        if (activeRadio !== null) closedRadios.add(activeRadio);
        if (radio !== null && closedRadios.has(radio)) fail(rules.consecutive);
        activeRadio = radio;
      }
      rules.visit(item, type, description, depth, tools);
    }
    for (const [group, checked] of checkedRadios) {
      if (checked !== 1) fail(`${rules.radioGroup} ${JSON.stringify(group)} must have exactly one checked item`);
    }
  };
  level(menu, 1);
}

function validateApplicationMenu(menu, fail) {
  const roles = new Set();
  validateMenuTree(menu, fail, {
    subject: "menu.menu",
    arrays: "application menus and their submenus must be arrays",
    depth: "application menus may be nested at most 16 levels",
    count: "application menus may contain at most 512 entries",
    consecutive: "items in a menu radio group must be consecutive at one menu level",
    duplicateId: "menu item ids must be unique across the whole menu tree",
    radioGroup: "menu radio group",
    typeOf: item => item.type ?? "action",
    // A menu bar holds submenus and nothing else: macOS refuses anything
    // else, and a bare command in a Windows menu bar fires on one click with
    // no menu ever opening.
    beforeItem: (type, depth) => {
      if (depth === 1 && type !== "submenu") {
        fail("every top-level entry of \"menu.menu\" must be a submenu");
      }
    },
    visit: (item, type, description, depth, t) => {
      if ("accelerator" in item) t.accelerator(item.accelerator, description);
      if (type === "submenu") {
        t.keys(item, ["type", "label", "enabled", "role", "menu"], description);
        t.nonEmpty(item.label, `${description}.label`);
        t.enabled(item, description);
        if ("role" in item) {
          if (depth !== 1) fail("only a top-level submenu of \"menu.menu\" carries a role");
          if (!menuSubmenuRoles.includes(item.role)) {
            fail(`${description}.role must be one of ${menuSubmenuRoles.join(", ")}`);
          }
          if (roles.has(item.role)) fail(`"menu.menu" declares the ${item.role} role twice`);
          roles.add(item.role);
        }
        if (!("menu" in item)) fail(`${description}.menu is required`);
        t.recurse(item.menu);
        return;
      }
      if (type === "role") {
        t.keys(item, ["type", "role"], description);
        if (!menuRoles.includes(item.role)) {
          fail(`${description}.role must be one of ${menuRoles.join(", ")}`);
        }
        return;
      }
      if (type === "separator") {
        t.keys(item, ["type"], description);
        return;
      }
      if (type !== "action" && type !== "checkbox" && type !== "radio") {
        fail(`${description}.type is unknown: ${JSON.stringify(type)}`);
      }
      t.keys(item, type === "action"
        ? ["type", "id", "label", "enabled", "accelerator"]
        : ["type", "id", "label", "enabled", "checked", "group", "accelerator"], description);
      t.nonEmpty(item.id, `${description}.id`);
      t.nonEmpty(item.label, `${description}.label`);
      t.uniqueId(item.id);
      t.enabled(item, description);
      if ("checked" in item && typeof item.checked !== "boolean") {
        fail(`${description}.checked must be a boolean`);
      }
      if (type === "checkbox" && "group" in item) {
        fail(`${description}.group is only valid on radio items`);
      }
      if (type === "radio") t.radio(item, description);
    },
  });
}

function validateTrayMenu(menu, fail) {
  let hasQuit = false;
  validateMenuTree(menu, fail, {
    subject: "tray.contextMenu",
    arrays: "tray context menus and submenu menus must be arrays",
    depth: "tray menus may be nested at most 16 levels",
    count: "tray menus may contain at most 512 entries",
    consecutive: "items in a tray radio group must be consecutive at one menu level",
    duplicateId: "tray menu item ids must be unique across the whole menu tree",
    radioGroup: "tray radio group",
    typeOf: item => item.type ?? (item.action === "separator" ? "separator" : "action"),
    visit: (item, type, description, depth, t) => {
      const common = () => {
        t.enabled(item, description);
        if ("accelerator" in item) t.accelerator(item.accelerator, description);
        if ("icon" in item) {
          t.nonEmpty(item.icon, `${description}.icon`);
          if (!/\.png$/.test(item.icon)) fail(`${description}.icon must name a PNG file`);
        }
      };
      if (type === "separator") {
        const legacy = item.action === "separator" && item.type === undefined;
        t.keys(item, legacy ? ["action", "label", "enabled"] : ["type"], description);
        if (!legacy && item.type !== "separator") fail(`${description} is an ambiguous separator`);
        if (legacy && "label" in item) t.nonEmpty(item.label, `${description}.label`);
        if (legacy && "enabled" in item && typeof item.enabled !== "boolean") {
          fail(`${description}.enabled must be a boolean`);
        }
        return;
      }
      if (type === "submenu") {
        t.keys(item, ["type", "label", "enabled", "icon", "menu"], description);
        t.nonEmpty(item.label, `${description}.label`);
        common();
        if (!("menu" in item)) fail(`${description}.menu is required`);
        t.recurse(item.menu);
        return;
      }
      if (type === "checkbox" || type === "radio") {
        t.keys(item, ["type", "id", "label", "enabled", "checked", "group", "accelerator"], description);
        t.nonEmpty(item.id, `${description}.id`);
        t.nonEmpty(item.label, `${description}.label`);
        t.uniqueId(item.id);
        if ("checked" in item && typeof item.checked !== "boolean") fail(`${description}.checked must be a boolean`);
        if (type === "checkbox" && "group" in item) fail(`${description}.group is only valid on radio items`);
        if (type === "radio") t.radio(item, description);
        common();
        return;
      }
      if (type !== "action") fail(`${description}.type is unknown: ${JSON.stringify(type)}`);
      t.keys(item, ["type", "id", "action", "label", "enabled", "accelerator", "icon"], description);
      const hasId = "id" in item;
      const hasAction = "action" in item;
      if (hasId === hasAction) fail(`${description} needs exactly one of id or action`);
      if (hasId) {
        t.nonEmpty(item.id, `${description}.id`);
        t.nonEmpty(item.label, `${description}.label`);
        t.uniqueId(item.id);
      } else if (!["show", "hide", "quit"].includes(item.action)) {
        fail(`${description}.action must be show, hide, or quit`);
      } else if (item.action === "quit") hasQuit = true;
      if ("label" in item) t.nonEmpty(item.label, `${description}.label`);
      common();
    },
  });
  return hasQuit;
}

/** Resolves package-relative tray assets and encodes menu icons by index. */
export async function recordTrayConfiguration(tray, root) {
  const packageRoot = await realpath(root);
  const asset = async (declared, description) => {
    if (isAbsolute(declared)) throw new Error(`${description} must be relative to package.json`);
    const unresolved = resolve(packageRoot, declared);
    const lexical = relative(packageRoot, unresolved);
    if (lexical.startsWith("..") || isAbsolute(lexical)) {
      throw new Error(`${description} escapes the package containing the configuration: ${declared}`);
    }
    const absolute = await realpath(unresolved).catch(() => {
      throw new Error(`${description} does not exist: ${declared}`);
    });
    const escaped = relative(packageRoot, absolute);
    if (escaped.startsWith("..") || isAbsolute(escaped)) {
      throw new Error(`${description} escapes the package containing the configuration: ${declared}`);
    }
    const bytes = await readFile(absolute).catch(error => {
      throw new Error(`${description} is not a readable file: ${declared}: ${error.message}`);
    });
    pngDimensions(bytes, declared);
    return absolute;
  };
  const menuIcons = [];
  const encode = async items => {
    const encodedItems = [];
    for (const item of items) {
      if (item.action === "separator" && item.type === undefined) {
        encodedItems.push({ type: "separator" });
        continue;
      }
      const encoded = { ...item };
      if (item.icon !== undefined) {
        encoded.iconIndex = menuIcons.length;
        menuIcons.push(await asset(item.icon, "a tray menu icon"));
        delete encoded.icon;
      }
      if (item.menu !== undefined) encoded.menu = await encode(item.menu);
      encodedItems.push(encoded);
    }
    return encodedItems;
  };
  return {
    ...tray,
    icon: await asset(tray.icon, "tray.icon"),
    contextMenu: await encode(tray.contextMenu ?? []),
    menuIcons,
  };
}

export function defineConfig(config) {
  return validateConfig(config, "defineConfig()");
}

// The nearest package.json declaring the key wins; the nearest one overall is
// remembered so "no config" can name the file the user probably meant.
export async function loadConfig(from = process.cwd()) {
  let directory = resolve(from);
  let nearest = null;
  for (;;) {
    const path = join(directory, "package.json");
    const source = await readFile(path, "utf8").catch(() => null);
    if (source !== null) {
      nearest ??= path;
      let manifest;
      try {
        manifest = JSON.parse(source);
      } catch (error) {
        throw new Error(`${path} is not valid JSON: ${error.message}`);
      }
      if (manifest.blitsen !== undefined) {
        return { path, root: directory, config: validateConfig(manifest.blitsen, path) };
      }
    }
    const parent = dirname(directory);
    if (parent === directory) return { path: nearest, root: null, config: null };
    directory = parent;
  }
}

// Structural constraint 6: this runs the command the user configured and consumes
// a directory. Blitsen never inspects the build tool, so the command is handed to
// the platform shell exactly as written, with the local .bin on PATH the way a
// package-manager script would see it.
export function runBuildCommand(command, cwd) {
  // Windows spells it Path, and a spread of process.env is a plain object, so the
  // existing key has to be replaced rather than a second one added.
  const key = Object.keys(process.env).find(name => name.toUpperCase() === "PATH") ?? "PATH";
  const env = {
    ...process.env,
    [key]: [join(cwd, "node_modules", ".bin"), process.env[key] ?? ""].join(delimiter),
  };
  return new Promise((settle, reject) => {
    const child = spawn(command, { cwd, shell: true, stdio: "inherit", env });
    child.on("error", error =>
      reject(new Error(`build command could not start: ${command} (${error.message})`)));
    child.on("close", (code, signal) => {
      if (signal) reject(new Error(`build command was terminated by ${signal}: ${command}`));
      else if (code !== 0) reject(new Error(`build command failed with exit code ${code}: ${command}`));
      else settle();
    });
  });
}
