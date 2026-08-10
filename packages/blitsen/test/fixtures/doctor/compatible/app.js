const root = document.getElementById("root");
root.textContent = "compatible";
root.addEventListener("click", () => requestAnimationFrame(() => root.classList.add("active")));
