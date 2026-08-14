import "./dom-matrix.js";
import { StrictMode, useCallback } from "react";
import { createRoot } from "react-dom/client";
import {
  addEdge,
  EdgeLabelRenderer,
  Handle,
  Panel,
  Position,
  ReactFlow,
  useEdgesState,
  useNodesState,
  useReactFlow,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import "./styles.css";

const initialNodes = [
  {
    id: "webhook",
    position: { x: 0, y: 220 },
    data: {
      icon: "→", tone: "orange", app: "Webhook", title: "Order received",
      description: "POST /orders/created", trigger: true,
    },
    type: "automation",
  },
  {
    id: "normalize",
    position: { x: 210, y: 220 },
    data: {
      icon: "{}", tone: "blue", app: "Code", title: "Normalize payload",
      description: "Map 14 order fields",
    },
    type: "automation",
  },
  {
    id: "high-value",
    position: { x: 420, y: 220 },
    data: {
      icon: "IF", tone: "violet", app: "Logic", title: "High value order?",
      description: "total > £500",
      outputs: [
        { id: "true", label: "true", top: "36%" },
        { id: "false", label: "false", top: "72%" },
      ],
    },
    type: "automation",
  },
  {
    id: "slack",
    position: { x: 630, y: 70 },
    data: {
      icon: "#", tone: "pink", app: "Slack", title: "Alert VIP desk",
      description: "Post to #priority-orders",
    },
    type: "automation",
  },
  {
    id: "coupon",
    position: { x: 840, y: 70 },
    data: {
      icon: "%", tone: "green", app: "Shopify", title: "Add loyalty credit",
      description: "Create £25 store credit",
    },
    type: "automation",
  },
  {
    id: "ship",
    position: { x: 630, y: 350 },
    data: {
      icon: "▣", tone: "cyan", app: "ShipStation", title: "Create shipment",
      description: "Royal Mail · Tracked 24",
    },
    type: "automation",
  },
  {
    id: "crm",
    position: { x: 840, y: 350 },
    data: {
      icon: "C", tone: "blue", app: "HubSpot", title: "Update customer",
      description: "Set lifecycle + order total",
    },
    type: "automation",
  },
  {
    id: "respond",
    position: { x: 1050, y: 220 },
    data: {
      icon: "✓", tone: "green", app: "Webhook", title: "Return success",
      description: "200 · workflow complete", terminal: true,
    },
    type: "automation",
  },
];

const initialEdges = [
  { id: "webhook-normalize", source: "webhook", target: "normalize", type: "html" },
  { id: "normalize-high-value", source: "normalize", target: "high-value", type: "html" },
  { id: "vip-slack", source: "high-value", sourceHandle: "true", target: "slack", type: "html", data: { tone: "true" } },
  { id: "slack-coupon", source: "slack", target: "coupon", type: "html", data: { tone: "true" } },
  { id: "regular-ship", source: "high-value", sourceHandle: "false", target: "ship", type: "html", data: { tone: "false" } },
  { id: "ship-crm", source: "ship", target: "crm", type: "html" },
  { id: "coupon-respond", source: "coupon", target: "respond", type: "html", data: { tone: "true" } },
  { id: "crm-respond", source: "crm", target: "respond", type: "html" },
];

function WorkflowNode({ data }) {
  const outputs = data.outputs ?? [{ id: null, top: "50%" }];

  return (
    <article className={`workflow-node tone-${data.tone}`}>
      {!data.trigger && <Handle type="target" position={Position.Left} />}
      <header className="node-heading">
        <span className="node-icon">{data.icon}</span>
        <span className="node-app">{data.app}</span>
        <span className="node-ok">✓</span>
      </header>
      <strong>{data.title}</strong>
      <small>{data.description}</small>
      <footer>
        <span>Executed</span>
        <span>24 ms</span>
      </footer>
      {!data.terminal && outputs.map(output => (
        <Handle
          key={output.id ?? "output"}
          id={output.id}
          type="source"
          position={Position.Right}
          style={{ top: output.top }}
        />
      ))}
      {data.outputs?.map(output => (
        <span key={output.id} className="output-label" style={{ top: output.top }}>
          {output.label}
        </span>
      ))}
    </article>
  );
}

// Blitz does not paint SVG paths yet. React Flow's custom-edge API lets the
// example keep a useful graph in Blitsen with an ordinary transformed div; a
// browser gets the same edge and interaction model.
function HtmlLine({ sourceX, sourceY, targetX, targetY, className = "" }) {
  const deltaX = targetX - sourceX;
  const deltaY = targetY - sourceY;
  const length = Math.hypot(deltaX, deltaY);
  const angle = Math.atan2(deltaY, deltaX);

  return (
    <EdgeLabelRenderer>
      <div
        className={`html-edge ${className}`}
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

function HtmlEdge({ sourceX, sourceY, targetX, targetY, data }) {
  return (
    <HtmlLine
      sourceX={sourceX}
      sourceY={sourceY}
      targetX={targetX}
      targetY={targetY}
      className={`edge-${data?.tone ?? "default"}`}
    />
  );
}

// React Flow's built-in connection preview is an SVG path. Keep the same live
// coordinates but portal an HTML line beside the custom HTML edges so Blitsen
// can paint the gesture while it is in progress.
function HtmlConnectionLine({ fromX, fromY, toX, toY, connectionStatus }) {
  return (
    <HtmlLine
      sourceX={fromX}
      sourceY={fromY}
      targetX={toX}
      targetY={toY}
      className={`connection-preview ${connectionStatus ?? "pending"}`}
    />
  );
}

const edgeTypes = { html: HtmlEdge };
const nodeTypes = { automation: WorkflowNode };

function TextControls() {
  const { fitView, zoomIn, zoomOut } = useReactFlow();
  return (
    <Panel className="text-controls" position="bottom-left">
      <button type="button" aria-label="Zoom in" onClick={() => zoomIn()}>+</button>
      <button type="button" aria-label="Zoom out" onClick={() => zoomOut()}>−</button>
      <button type="button" onClick={() => fitView({ padding: 0.12 })}>Fit workflow</button>
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
      <header className="workflow-header">
        <div className="workflow-identity">
          <span className="brand-mark">B</span>
          <div>
            <small>Commerce ops / Production</small>
            <h1>Fulfil high-value orders</h1>
          </div>
        </div>
        <div className="workflow-actions">
          <span className="save-state">Saved just now</span>
          <span className="active-state"><i /> Active</span>
          <button type="button">Execute workflow</button>
        </div>
      </header>
      <div className="flow-area">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          connectionLineComponent={HtmlConnectionLine}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          fitView
          fitViewOptions={{ padding: 0.12 }}
          minZoom={0.45}
          maxZoom={1.5}
          attributionPosition="bottom-left"
        >
          <TextControls />
          <Panel className="run-summary" position="top-right">
            <span className="pulse" />
            Last run succeeded
            <small>8 of 8 steps · 1.2 s</small>
          </Panel>
        </ReactFlow>
      </div>
    </section>
  );
}

createRoot(document.getElementById("root")).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
