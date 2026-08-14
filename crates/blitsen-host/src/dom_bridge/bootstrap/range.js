  // Ranges, carets and the selection.
  //
  // A range is two boundary points into the tree, and everything else about one
  // is derived: which nodes it touches, what text that is, and where the text
  // sits on the screen. Geometry is the reason this exists — an editor measures
  // a run of characters by putting a range around it and asking — and it is the
  // one part a script cannot compute for itself, so it is the part the backend
  // answers. `textRects` measures a run inside a single text node; a range that
  // spans several asks once per node and returns what each of them occupies.
  //
  // What a range here is not is a way to mutate the tree. `deleteContents`,
  // `extractContents`, `insertNode` and `surroundContents` all split a text node
  // at a boundary, and this runtime has no `splitText` and no character-data
  // interface to split one with. They are absent rather than half-built, and a
  // caller that reaches for one gets a `TypeError` on an undefined method rather
  // than a tree quietly cut in the wrong place.

  const rangeBounds = new WeakMap();
  const rangeBetween = (startNode, startOffset, endNode, endOffset) => {
    const range = new Range();
    range.setStart(startNode, startOffset);
    range.setEnd(endNode, endOffset);
    return range;
  };
  // How many code units or children a boundary point may address in a node.
  const nodeLength = node =>
    node.nodeType === 3 || node.nodeType === 8 ? node.textContent.length : node.childNodes.length;
  const childIndexOf = node => {
    const parent = node.parentNode;
    return parent === null ? -1 : [...parent.childNodes].indexOf(node);
  };
  // A boundary point beside a node is a point in its parent, so a node without
  // one cannot have a range placed around it.
  const parentOf = node => {
    requireNode(node);
    const parent = node.parentNode;
    if (parent === null) throw new TypeError("the node has no parent to place a boundary in");
    return parent;
  };
  const ancestorsOf = node => {
    const chain = [];
    for (let current = node; current; current = current.parentNode) chain.unshift(current);
    return chain;
  };
  // Where one boundary point sits relative to another: -1 before, 0 the same
  // place, 1 after, and `null` for two points in different trees, which have no
  // order at all. Two points in the same node compare by offset; otherwise the
  // comparison is made at the deepest node that contains both of them, where it
  // is a comparison of child positions — and a point *in* an ancestor is before
  // or after a descendant according to which side of that child it names.
  const comparePoints = (node, offset, other, otherOffset) => {
    if (node === other) return offset === otherOffset ? 0 : offset < otherOffset ? -1 : 1;
    const mine = ancestorsOf(node);
    const theirs = ancestorsOf(other);
    if (mine[0] !== theirs[0]) return null;
    let depth = 0;
    while (mine[depth] === theirs[depth]) depth += 1;
    if (depth === mine.length) return childIndexOf(theirs[depth]) < offset ? 1 : -1;
    if (depth === theirs.length) return childIndexOf(mine[depth]) < otherOffset ? -1 : 1;
    const siblings = [...mine[depth - 1].childNodes];
    return siblings.indexOf(mine[depth]) < siblings.indexOf(theirs[depth]) ? -1 : 1;
  };

  const boundsOf = range => {
    const bounds = rangeBounds.get(range);
    if (!bounds) throw new TypeError("not a Range");
    return bounds;
  };
  const requireOffset = (node, offset) => {
    offset = Math.trunc(Number(offset)) || 0;
    if (offset < 0 || offset > nodeLength(node))
      throw new RangeError("offset is outside the node");
    return offset;
  };
  // Moving one boundary past the other collapses the range onto the point that
  // moved, which is what keeps a range's start before its end at all times.
  const setBound = (range, which, node, offset) => {
    requireNode(node);
    const bounds = boundsOf(range);
    const other = which === "start" ? "end" : "start";
    bounds[`${which}Container`] = node;
    bounds[`${which}Offset`] = offset;
    const order = comparePoints(bounds.startContainer, bounds.startOffset,
      bounds.endContainer, bounds.endOffset);
    if (order === null || order > 0) {
      bounds[`${other}Container`] = node;
      bounds[`${other}Offset`] = offset;
    }
  };

  // Every node a range reaches, in tree order. Pruned rather than filtered: a
  // subtree the range does not touch is not walked into, so measuring a range
  // inside one paragraph does not visit the rest of the document.
  const touchedNodes = range => {
    const bounds = boundsOf(range);
    const found = [];
    const visit = node => {
      found.push(node);
      for (const child of node.childNodes) if (touches(bounds, child)) visit(child);
    };
    visit(range.commonAncestorContainer);
    return found;
  };
  const touches = (bounds, node) => {
    const parent = node.parentNode;
    if (parent === null) return false;
    const index = childIndexOf(node);
    return comparePoints(parent, index, bounds.endContainer, bounds.endOffset) < 0
      && comparePoints(parent, index + 1, bounds.startContainer, bounds.startOffset) > 0;
  };
  const contains = (bounds, node) => {
    const parent = node.parentNode;
    if (parent === null) return false;
    const index = childIndexOf(node);
    return comparePoints(parent, index, bounds.startContainer, bounds.startOffset) >= 0
      && comparePoints(parent, index + 1, bounds.endContainer, bounds.endOffset) <= 0;
  };
  // The part of a text node a range covers, as offsets into its data.
  const coveredText = (bounds, node) => [
    node === bounds.startContainer ? bounds.startOffset : 0,
    node === bounds.endContainer ? bounds.endOffset : nodeLength(node),
  ];

  class Range {
    // A new range is collapsed at the start of the document, and so measures
    // nothing until it is pointed at something.
    constructor() {
      rangeBounds.set(this, { startContainer: document, startOffset: 0,
        endContainer: document, endOffset: 0 });
    }
    get startContainer() { return boundsOf(this).startContainer; }
    get startOffset() { return boundsOf(this).startOffset; }
    get endContainer() { return boundsOf(this).endContainer; }
    get endOffset() { return boundsOf(this).endOffset; }
    get collapsed() {
      const bounds = boundsOf(this);
      return bounds.startContainer === bounds.endContainer
        && bounds.startOffset === bounds.endOffset;
    }
    // The deepest node containing both boundary points, which is the subtree
    // the range is entirely inside.
    get commonAncestorContainer() {
      const bounds = boundsOf(this);
      const start = ancestorsOf(bounds.startContainer);
      const end = ancestorsOf(bounds.endContainer);
      let depth = 0;
      while (start[depth] && start[depth] === end[depth]) depth += 1;
      return start[depth - 1] ?? bounds.startContainer;
    }
    setStart(node, offset) { setBound(this, "start", node, requireOffset(node, offset)); }
    setEnd(node, offset) { setBound(this, "end", node, requireOffset(node, offset)); }
    setStartBefore(node) { setBound(this, "start", parentOf(node), childIndexOf(node)); }
    setStartAfter(node) { setBound(this, "start", parentOf(node), childIndexOf(node) + 1); }
    setEndBefore(node) { setBound(this, "end", parentOf(node), childIndexOf(node)); }
    setEndAfter(node) { setBound(this, "end", parentOf(node), childIndexOf(node) + 1); }
    collapse(toStart = false) {
      const bounds = boundsOf(this);
      if (toStart) setBound(this, "end", bounds.startContainer, bounds.startOffset);
      else setBound(this, "start", bounds.endContainer, bounds.endOffset);
    }
    selectNode(node) {
      this.setStartBefore(node);
      this.setEndAfter(node);
    }
    selectNodeContents(node) {
      requireNode(node);
      this.setStart(node, 0);
      this.setEnd(node, nodeLength(node));
    }
    cloneRange() {
      const copy = new Range();
      const bounds = boundsOf(this);
      copy.setStart(bounds.startContainer, bounds.startOffset);
      copy.setEnd(bounds.endContainer, bounds.endOffset);
      return copy;
    }
    // Legacy, and a no-op in every engine that still answers it: there is
    // nothing to detach a range from.
    detach() {}
    comparePoint(node, offset) {
      const bounds = boundsOf(this);
      offset = requireOffset(node, offset);
      if (comparePoints(node, offset, bounds.startContainer, bounds.startOffset) < 0) return -1;
      if (comparePoints(node, offset, bounds.endContainer, bounds.endOffset) > 0) return 1;
      return 0;
    }
    isPointInRange(node, offset) {
      if (!(node instanceof Node) || node.getRootNode() !== this.commonAncestorContainer.getRootNode())
        return false;
      return this.comparePoint(node, offset) === 0;
    }
    intersectsNode(node) {
      requireNode(node);
      return touches(boundsOf(this), node) || node === this.commonAncestorContainer;
    }
    compareBoundaryPoints(how, other) {
      const ends = [["start", "start"], ["start", "end"], ["end", "end"], ["end", "start"]][how];
      if (!ends) throw new TypeError("invalid boundary-point comparison");
      const [here, there] = ends;
      const mine = boundsOf(this);
      const theirs = boundsOf(other);
      const order = comparePoints(mine[`${here}Container`], mine[`${here}Offset`],
        theirs[`${there}Container`], theirs[`${there}Offset`]);
      if (order === null) throw new TypeError("the ranges are in different trees");
      return order;
    }
    // The text the range covers, which is every text node it touches with the
    // two partially covered ones clipped to the boundary points.
    toString() {
      const bounds = boundsOf(this);
      let text = "";
      for (const node of touchedNodes(this)) {
        if (node.nodeType !== 3) continue;
        const [from, to] = coveredText(bounds, node);
        text += node.textContent.slice(from, to);
      }
      return text;
    }
    // Where the range is on the screen: the box each text run occupies, one per
    // line box it was broken across, and the border boxes of the elements the
    // range covers whole. A collapsed range covers nothing and so has none —
    // measure a character rather than a caret to find where a caret goes.
    getClientRects() {
      if (this.collapsed) return Object.freeze([]);
      const bounds = boundsOf(this);
      const rects = [];
      for (const node of touchedNodes(this)) {
        if (node.nodeType === 3) {
          const [from, to] = coveredText(bounds, node);
          if (to > from)
            rects.push(...recordForcedLayout(
              call("textRects", node[handle], from, to)).rects);
        } else if (node.nodeType === 1 && contains(bounds, node)) {
          rects.push(...recordForcedLayout(call("clientRects", node[handle])).rects);
        }
      }
      return Object.freeze(rects.map(rect => clientRect(rect.x, rect.y, rect.width, rect.height)));
    }
    // The union of those, and an empty box when there are none — which is what
    // a caller measuring a range that laid nothing out reads.
    getBoundingClientRect() {
      const rects = this.getClientRects();
      if (rects.length === 0) return clientRect(0, 0, 0, 0);
      const left = Math.min(...rects.map(rect => rect.left));
      const top = Math.min(...rects.map(rect => rect.top));
      const right = Math.max(...rects.map(rect => rect.right));
      const bottom = Math.max(...rects.map(rect => rect.bottom));
      return clientRect(left, top, right - left, bottom - top);
    }
  }
  Object.defineProperty(Range.prototype, Symbol.toStringTag,
    { value: "Range", configurable: true });
  // The four boundary-point comparisons, which the DOM numbers from zero.
  defineConstants(Range, ["START_TO_START", "START_TO_END", "END_TO_END", "END_TO_START"]);

  // A character boundary the way `caretPositionFromPoint` reports one: the text
  // node the point landed in, and how far into it.
  const caretPositions = new WeakMap();
  class CaretPosition {
    constructor(node, offset) {
      caretPositions.set(this, { offsetNode: node, offset });
    }
    get offsetNode() { return caretPositions.get(this).offsetNode; }
    get offset() { return caretPositions.get(this).offset; }
    // A zero-width box where the caret would be drawn. There is no caret in the
    // layout to read, so it is measured off the character beside it: the left
    // edge of the one it sits in front of, or the right edge of the last one
    // when it sits at the end of the text.
    getClientRect() {
      const node = this.offsetNode;
      const length = nodeLength(node);
      const trailing = this.offset >= length;
      const rects = trailing
        ? rangeBetween(node, Math.max(0, length - 1), node, length).getClientRects()
        : rangeBetween(node, this.offset, node, this.offset + 1).getClientRects();
      if (rects.length === 0) return null;
      const rect = trailing ? rects[rects.length - 1] : rects[0];
      return clientRect(trailing ? rect.right : rect.left, rect.top, 0, rect.height);
    }
  }

  // The selection, which is one object for the life of the document: a caller
  // holds on to what `getSelection` returned and expects it to keep up.
  //
  // It holds an anchor and a focus rather than a range, because those carry the
  // direction a range cannot — a selection made backwards has its focus before
  // its anchor, and `direction` is what an editor reads to know which end the
  // user is dragging. Nothing here paints: this is the selection a script sets
  // and reads, and the renderer draws no highlight for it.
  const selection = { anchorNode: null, anchorOffset: 0, focusNode: null, focusOffset: 0 };
  let selectionChangePending = false;
  // Announced in a later task rather than from inside the call that changed it,
  // so a listener sees the selection settled and a run of changes says so once.
  const selectionChanged = () => {
    if (selectionChangePending) return;
    selectionChangePending = true;
    hostSetTimeout(() => {
      selectionChangePending = false;
      document.dispatchEvent(new Event("selectionchange"));
    }, 0);
  };
  const selectionRange = () => {
    if (selection.anchorNode === null || selection.focusNode === null) return null;
    const order = comparePoints(selection.anchorNode, selection.anchorOffset,
      selection.focusNode, selection.focusOffset);
    if (order === null) return null;
    return order <= 0
      ? rangeBetween(selection.anchorNode, selection.anchorOffset,
        selection.focusNode, selection.focusOffset)
      : rangeBetween(selection.focusNode, selection.focusOffset,
        selection.anchorNode, selection.anchorOffset);
  };

  class Selection {
    get anchorNode() { return selection.anchorNode; }
    get anchorOffset() { return selection.anchorOffset; }
    get focusNode() { return selection.focusNode; }
    get focusOffset() { return selection.focusOffset; }
    get rangeCount() { return selection.anchorNode === null ? 0 : 1; }
    get isCollapsed() {
      return selection.anchorNode === null
        || (selection.anchorNode === selection.focusNode
          && selection.anchorOffset === selection.focusOffset);
    }
    get type() {
      return selection.anchorNode === null ? "None" : this.isCollapsed ? "Caret" : "Range";
    }
    // Which end the selection grew from, and "none" when there is nothing to
    // have grown or the two ends are in the same place.
    get direction() {
      if (selection.anchorNode === null || this.isCollapsed) return "none";
      const order = comparePoints(selection.anchorNode, selection.anchorOffset,
        selection.focusNode, selection.focusOffset);
      return order === null ? "none" : order < 0 ? "forward" : "backward";
    }
    // A copy of the selection rather than the live range a browser hands back:
    // writing to that range would move the selection, and there is nothing in
    // this runtime a live range could be observed through that a second call to
    // this cannot answer.
    getRangeAt(index) {
      const range = Number(index) === 0 ? selectionRange() : null;
      if (range === null) throw new RangeError("there is no range at that index");
      return range;
    }
    addRange(range) {
      if (this.rangeCount > 0) return;
      const bounds = boundsOf(range);
      this.setBaseAndExtent(bounds.startContainer, bounds.startOffset,
        bounds.endContainer, bounds.endOffset);
    }
    removeAllRanges() {
      if (selection.anchorNode === null) return;
      selection.anchorNode = selection.focusNode = null;
      selection.anchorOffset = selection.focusOffset = 0;
      selectionChanged();
    }
    empty() { this.removeAllRanges(); }
    setBaseAndExtent(anchorNode, anchorOffset, focusNode, focusOffset) {
      requireNode(anchorNode);
      requireNode(focusNode);
      selection.anchorNode = anchorNode;
      selection.anchorOffset = requireOffset(anchorNode, anchorOffset);
      selection.focusNode = focusNode;
      selection.focusOffset = requireOffset(focusNode, focusOffset);
      selectionChanged();
    }
    collapse(node, offset = 0) {
      if (node === null) return this.removeAllRanges();
      this.setBaseAndExtent(node, offset, node, offset);
    }
    setPosition(node, offset = 0) { this.collapse(node, offset); }
    collapseToStart() {
      const range = selectionRange();
      if (range === null) throw new Error("there is no selection to collapse");
      this.collapse(range.startContainer, range.startOffset);
    }
    collapseToEnd() {
      const range = selectionRange();
      if (range === null) throw new Error("there is no selection to collapse");
      this.collapse(range.endContainer, range.endOffset);
    }
    // Moves the focus and leaves the anchor, which is what makes a selection
    // have a direction at all.
    extend(node, offset = 0) {
      if (selection.anchorNode === null) throw new Error("there is nothing to extend");
      this.setBaseAndExtent(selection.anchorNode, selection.anchorOffset, node, offset);
    }
    selectAllChildren(node) {
      requireNode(node);
      this.setBaseAndExtent(node, 0, node, nodeLength(node));
    }
    containsNode(node, allowPartial = false) {
      const range = selectionRange();
      if (range === null || !(node instanceof Node)) return false;
      return allowPartial ? range.intersectsNode(node) : contains(boundsOf(range), node);
    }
    toString() { return selectionRange()?.toString() ?? ""; }
  }
  Object.defineProperty(Selection.prototype, Symbol.toStringTag,
    { value: "Selection", configurable: true });

  const documentSelection = new Selection();
  const getSelection = () => documentSelection;
