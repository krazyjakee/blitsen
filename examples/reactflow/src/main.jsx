import "./dom-matrix.js";
import { StrictMode, useCallback } from "react";
import { createRoot } from "react-dom/client";
import {
  addEdge,
  EdgeLabelRenderer,
  Panel,
  ReactFlow,
  useEdgesState,
  useNodesState,
  useReactFlow,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import "./styles.css";

const initialNodes = [
  {
    id: "source",
    position: { x: 80, y: 80 },
    data: { label: "Browser-free UI" },
    type: "input",
  },
  {
    id: "flow",
    position: { x: 350, y: 210 },
    data: { label: "React Flow" },
  },
  {
    id: "native",
    position: { x: 620, y: 80 },
    data: { label: "Native window" },
    type: "output",
  },
];

const initialEdges = [
  {
    id: "source-flow",
    source: "source",
    target: "flow",
    type: "html",
    animated: true,
  },
  {
    id: "flow-native",
    source: "flow",
    target: "native",
    type: "html",
    animated: true,
  },
];

// Blitz does not paint SVG paths yet. React Flow's custom-edge API lets the
// example keep a useful graph in Blitsen with an ordinary transformed div; a
// browser gets the same edge and interaction model.
function HtmlEdge({ sourceX, sourceY, targetX, targetY }) {
  const deltaX = targetX - sourceX;
  const deltaY = targetY - sourceY;
  const length = Math.hypot(deltaX, deltaY);
  const angle = Math.atan2(deltaY, deltaX);

  return (
    <EdgeLabelRenderer>
      <div
        className="html-edge"
        style={{
          width: length,
          transform: `translate(${sourceX}px, ${sourceY}px) rotate(${angle}rad)`,
        }}
      >
        <i />
      </div>
    </EdgeLabelRenderer>
  );
}

const edgeTypes = { html: HtmlEdge };

function TextControls() {
  const { fitView, zoomIn, zoomOut } = useReactFlow();
  return (
    <Panel className="text-controls" position="bottom-left">
      <button type="button" aria-label="Zoom in" onClick={() => zoomIn()}>+</button>
      <button type="button" aria-label="Zoom out" onClick={() => zoomOut()}>−</button>
      <button type="button" onClick={() => fitView()}>Fit</button>
    </Panel>
  );
}

function App() {
  const [nodes, setNodes, onNodesChange] = useNodesState(initialNodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges);
  const onConnect = useCallback(
    connection => setEdges(current => addEdge({ ...connection, type: "html" }, current)),
    [setEdges],
  );

  return (
    <section className="canvas" data-react-flow-ready="true">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        edgeTypes={edgeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        fitView
        minZoom={0.5}
        maxZoom={1.5}
        attributionPosition="bottom-left"
      >
        <TextControls />
        <aside className="title-card">
          <span>Blitsen example</span>
          <h1>React Flow</h1>
          <p>Drag a node, pan the canvas, or connect two handles.</p>
        </aside>
      </ReactFlow>
    </section>
  );
}

createRoot(document.getElementById("root")).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
