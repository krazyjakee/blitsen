import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import windowApi from 'blitsen/window';
import dialog from 'blitsen/dialog';

// Available even during startup in explicit UI test mode.
windowApi.setTitle?.('Application UI acceptance');
windowApi.setDecorations(false);
windowApi.setDecorations(true);
globalThis.dialogResult = 'pending';
dialog.openFolder().then(value => { globalThis.dialogResult = value; });
globalThis.submits = 0;
globalThis.loaded = false;
addEventListener('load', () => { globalThis.loaded = true; });
function App() {
  const [folder, setFolder] = useState('');
  const [pressed, setPressed] = useState(false);
  const [cover, setCover] = useState(false);
  const [saved, setSaved] = useState('');
  return <main className="app"><form onSubmit={event => {
    event.preventDefault(); globalThis.submits++; setSaved(folder);
  }}>
    <label htmlFor="folder">Local folder</label>
    <input id="folder" value={folder} onInput={event => setFolder(event.currentTarget.value)} />
    <button type="button" id="cover-switch" onClick={() => setCover(value => !value)}>Toggle cover</button>
    <div style={{ height: 825 }} />
    <div className="row" style={{ position: 'relative' }}>
      <button id="save" disabled={!folder} aria-pressed={pressed}
        onMouseDown={() => setPressed(true)}>Save repository</button>
      {cover && <div id="cover" style={{ position: 'absolute', inset: 0, zIndex: 20, background: 'red' }} />}
    </div>
    <output id="saved">{saved}</output>
  </form></main>;
}
createRoot(document.getElementById('root')).render(<App />);
