  // The shape a web IDL attribute has, and the one thing every scope needs
  // before anything else in it.
  //
  // An interface's members are read-only and enumerable: `Object.keys` and a
  // spread see them, and an assignment does not replace them. That is a property
  // descriptor rather than a field, and written out per member it is three
  // tokens of ceremony around one value, repeated once for every member of every
  // class below. This says the same thing once.
  //
  // Not `Object.assign` and not a field: both would make the member writable,
  // and a library that assigns over `event.data` would then be changing what the
  // next listener reads rather than being ignored.
  //
  // Members that are deliberately *not* read-only — `detail`, which
  // `initCustomEvent` replaces — keep their own `Object.defineProperty` call,
  // because the descriptor is the point at those.
  const defineMembers = (target, members) => {
    const descriptors = {};
    for (const name of Reflect.ownKeys(members)) {
      descriptors[name] = { value: members[name], enumerable: true };
    }
    return Object.defineProperties(target, descriptors);
  };
