  // Intl. The classes are here and the data is native (see `intl/mod.rs`): what
  // this half does is the specification's argument handling — locale
  // negotiation, option reading, coercion — and nothing that needs CLDR.
  //
  // A number crosses as the decimal text JavaScript would print rather than as
  // a double, because rounding a currency to its minor units is a decimal
  // operation and doing it on the binary value is where the cent goes missing.
  const intlStates = new WeakMap();
  const intlState = (formatter, kind) => {
    const state = intlStates.get(formatter);
    if (!state || state.kind !== kind) throw new TypeError("Illegal invocation");
    return state;
  };
  // A tag is well-formed if it parses. Nothing here rejects a tag the data has
  // no entry for: CLDR falls back — `de-DE-u-x-private` formats as German —
  // which is what a browser does too.
  const LANGUAGE_TAG = /^[a-z]{2,8}(-[a-z0-9]{2,8})*$/i;
  const canonicalLocales = requested => {
    if (requested === undefined) return [];
    const list = typeof requested === "string" ? [requested] : Array.from(requested);
    return list.map(tag => {
      const text = String(tag);
      if (!LANGUAGE_TAG.test(text))
        throw new RangeError(`${text} is not a structurally valid language tag`);
      return text;
    });
  };
  // The first tag asked for, or the host's own when nothing was. Resolution
  // proper is the native side's: it is the only half that knows what data
  // exists to fall back through.
  const requestedLocale = locales => canonicalLocales(locales)[0] ?? "";
  const intlOptions = (locales, options, extra) => {
    const bag = { locale: requestedLocale(locales) };
    for (const [key, value] of Object.entries(options ?? {}))
      if (value !== undefined) bag[key] = value;
    return Object.assign(bag, extra);
  };
  const resolveIntl = (kind, options) => {
    const { handle, resolved } = JSON.parse(
      __blitsenIntlResolve(kind, JSON.stringify(options)));
    return { kind, handle: String(handle), resolved };
  };
  // `1e21` and `0.000001` print in exponential notation, and a decimal parser
  // is owed digits. Expanding here rather than natively keeps the boundary a
  // plain decimal string in every case.
  const decimalText = value => {
    const number = Number(value);
    if (!Number.isFinite(number)) throw new RangeError(`${value} cannot be formatted`);
    const text = String(number);
    const exponent = /^(-?)(\d+)(?:\.(\d+))?e([+-]\d+)$/i.exec(text);
    if (!exponent) return text;
    const [, sign, whole, fraction = "", power] = exponent;
    const digits = whole + fraction;
    const point = whole.length + Number(power);
    if (point <= 0) return `${sign}0.${"0".repeat(-point)}${digits}`;
    if (point >= digits.length) return `${sign}${digits}${"0".repeat(point - digits.length)}`;
    return `${sign}${digits.slice(0, point)}.${digits.slice(point)}`;
  };
  // Every constructor takes the same two arguments and answers the same two
  // questions, so `supportedLocalesOf` is one implementation: a tag is
  // supported when it is well formed, because the fallback behind it always
  // produces a formatting.
  const supportedLocalesOf = locales => canonicalLocales(locales);

  class NumberFormat {
    constructor(locales, options) {
      if (!new.target) return new NumberFormat(locales, options);
      const bag = intlOptions(locales, options);
      if (bag.style === "currency" && bag.currency === undefined)
        throw new TypeError("currency is required when style is \"currency\"");
      intlStates.set(this, resolveIntl("number", bag));
    }
    format(value) {
      const state = intlState(this, "number");
      return __blitsenIntlFormat(state.handle, decimalText(value));
    }
    resolvedOptions() { return { ...intlState(this, "number").resolved }; }
    static supportedLocalesOf(locales) { return supportedLocalesOf(locales); }
  }

  class DateTimeFormat {
    constructor(locales, options) {
      if (!new.target) return new DateTimeFormat(locales, options);
      intlStates.set(this, resolveIntl("datetime", intlOptions(locales, options)));
    }
    format(value) {
      const state = intlState(this, "datetime");
      const time = value === undefined ? Date.now() : Number(value);
      if (!Number.isFinite(time)) throw new RangeError("invalid time value");
      return __blitsenIntlFormat(state.handle, String(Math.trunc(time)));
    }
    resolvedOptions() { return { ...intlState(this, "datetime").resolved }; }
    static supportedLocalesOf(locales) { return supportedLocalesOf(locales); }
  }

  class RelativeTimeFormat {
    constructor(locales, options) {
      if (!new.target) throw new TypeError("Intl.RelativeTimeFormat requires new");
      intlStates.set(this, resolveIntl("relativetime", intlOptions(locales, options)));
    }
    format(value, unit) {
      const state = intlState(this, "relativetime");
      return __blitsenIntlFormat(state.handle, `${decimalText(value)}\u001f${String(unit)}`);
    }
    resolvedOptions() { return { ...intlState(this, "relativetime").resolved }; }
    static supportedLocalesOf(locales) { return supportedLocalesOf(locales); }
  }

  class PluralRules {
    constructor(locales, options) {
      if (!new.target) throw new TypeError("Intl.PluralRules requires new");
      intlStates.set(this, resolveIntl("plural", intlOptions(locales, options)));
    }
    select(value) {
      const state = intlState(this, "plural");
      return __blitsenIntlSelect(state.handle, decimalText(value));
    }
    resolvedOptions() { return { ...intlState(this, "plural").resolved }; }
    static supportedLocalesOf(locales) { return supportedLocalesOf(locales); }
  }

  class Collator {
    constructor(locales, options) {
      if (!new.target) return new Collator(locales, options);
      const state = resolveIntl("collator", intlOptions(locales, options));
      intlStates.set(this, state);
      // `compare` is a bound accessor in the specification, because it is
      // routinely handed straight to `Array.prototype.sort`.
      state.compare = (left, right) =>
        __blitsenIntlCompare(state.handle, String(left), String(right));
    }
    get compare() { return intlState(this, "collator").compare; }
    resolvedOptions() { return { ...intlState(this, "collator").resolved }; }
    static supportedLocalesOf(locales) { return supportedLocalesOf(locales); }
  }

  class ListFormat {
    constructor(locales, options) {
      if (!new.target) throw new TypeError("Intl.ListFormat requires new");
      intlStates.set(this, resolveIntl("list", intlOptions(locales, options)));
    }
    format(items) {
      const state = intlState(this, "list");
      return __blitsenIntlJoin(state.handle,
        JSON.stringify(Array.from(items ?? [], String)));
    }
    resolvedOptions() { return { ...intlState(this, "list").resolved }; }
    static supportedLocalesOf(locales) { return supportedLocalesOf(locales); }
  }

  const Intl = Object.freeze({
    NumberFormat, DateTimeFormat, RelativeTimeFormat, PluralRules, Collator, ListFormat,
    getCanonicalLocales: canonicalLocales,
  });

  // The three prototype methods that are `Intl` in disguise. The engine ships
  // its own, which ignore the locale they are given and return the invariant
  // form — the failure mode the manifest calls out, because it is silently
  // wrong output rather than a missing function. These replace them.
  const installIntlPrototypes = () => {
    Object.defineProperty(Number.prototype, "toLocaleString", {
      value: function toLocaleString(locales, options) {
        return new NumberFormat(locales, options).format(Number(this));
      },
      writable: true, configurable: true, enumerable: false,
    });
    // The defaults apply only when the caller named no field of its own: an
    // explicit `{ hour, minute }` means those two fields and not those two on
    // top of a medium time, which is what merging would produce.
    const FIELDS = ["dateStyle", "timeStyle", "weekday", "era", "year", "month", "day",
      "dayPeriod", "hour", "minute", "second", "fractionalSecondDigits", "timeZoneName"];
    const dateFormat = (value, locales, options, defaults) => {
      const time = Number(value);
      if (!Number.isFinite(time)) return "Invalid Date";
      const asked = options ?? {};
      const named = FIELDS.some(field => asked[field] !== undefined);
      return new DateTimeFormat(locales, named ? asked : { ...defaults, ...asked }).format(time);
    };
    for (const [name, defaults] of [
      ["toLocaleString", { dateStyle: "medium", timeStyle: "medium" }],
      ["toLocaleDateString", { dateStyle: "medium" }],
      ["toLocaleTimeString", { timeStyle: "medium" }],
    ])
      Object.defineProperty(Date.prototype, name, {
        value: { [name]: function (locales, options) {
          return dateFormat(this.getTime(), locales, options, defaults);
        } }[name],
        writable: true, configurable: true, enumerable: false,
      });
    Object.defineProperty(String.prototype, "localeCompare", {
      value: function localeCompare(that, locales, options) {
        return new Collator(locales, options).compare(String(this), String(that));
      },
      writable: true, configurable: true, enumerable: false,
    });
  };
