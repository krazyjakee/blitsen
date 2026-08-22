// WebGL is the context this runtime does not implement; the 2D one it does.
// The page still stands with the fallback, which is why this is graded a
// warning rather than an error (issue #99).
const context = document.getElementById("scene").getContext("webgl");
if (!context) document.getElementById("scene").getContext("2d").fillRect(0, 0, 10, 10);
localStorage.setItem("renderer", context ? "webgl" : "none");
