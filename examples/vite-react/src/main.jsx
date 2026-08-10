import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import "./style.css";

function App() {
  const [count, setCount] = useState(0);

  return (
    <main className="shell" data-react-ready="true">
      <header>
        <p className="eyebrow">Vite + React</p>
        <h1>Compatible adoption</h1>
        <p>An ordinary production bundle, rendered without a browser engine.</p>
      </header>
      <section className="cards" aria-label="Milestone status">
        <article>
          <strong>Framework</strong>
          <span>React</span>
        </article>
        <article>
          <strong>Bundler</strong>
          <span>Vite</span>
        </article>
        <article>
          <strong>Clicks</strong>
          <span id="count">{count}</span>
        </article>
      </section>
      <button id="increment" type="button" onClick={event => {
        event.currentTarget.setAttribute("data-clicked", "true");
        setCount(value => value + 1);
      }}>
        Increment
      </button>
    </main>
  );
}

createRoot(document.getElementById("root")).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
