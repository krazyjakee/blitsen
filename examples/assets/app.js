// Reports the boxes layout actually resolved, which is the observable half of
// image decoding: a `<img>` given only a width takes its height from the
// decoded intrinsic ratio, and one whose source never arrives takes nothing.
//
// `naturalWidth`, `naturalHeight` and `complete` are not read here because the
// bridge does not expose them yet; the resolved box is what this build can
// honestly show.
const box = (element) => {
  const rect = element.getBoundingClientRect();
  return `${Math.round(rect.width)} x ${Math.round(rect.height)}`;
};

document.getElementById("gradient-box").textContent =
  `${box(document.getElementById("gradient"))} from a 320x180 decode`;
document.getElementById("broken-box").textContent =
  `${box(document.getElementById("broken"))} — nothing decoded`;
