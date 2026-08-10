const context = document.getElementById("scene").getContext("webgl");
localStorage.setItem("renderer", context ? "webgl" : "none");
