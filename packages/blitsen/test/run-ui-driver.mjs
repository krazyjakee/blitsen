import assert from 'node:assert/strict';
import { launch } from '../src/test.mjs';

const app = await launch(process.argv[2], { width: 1180, height: 780,
  env: { DISPLAY: '', WAYLAND_DISPLAY: '', BLITSEN_AUDIO_OFFLINE: '1' },
  artifactsDir: process.argv[3], timeout: 20_000 });
try {
  await app.waitFor(() => globalThis.loaded && document.querySelector('#save'));
  await app.assert(() => globalThis.dialogResult === null);
  await app.assert(() => !('__blitsenInjectMouseEvent' in globalThis) && !('__blitsenInjectPointerAt' in globalThis));
  const save = { role: 'button', name: 'Save repository' };
  const field = { role: 'textbox', name: 'Local folder' };
  assert.equal((await app.query(save))[0].disabled, true);
  await assert.rejects(app.click(save), error => {
    assert.match(error.message, /disabled/);
    assert.equal(error.screenshot.subarray(1, 4).toString(), 'PNG');
    assert.ok(Array.isArray(error.events));
    return true;
  });
  await app.click(field);
  assert.equal((await app.query(field))[0].focused, true);
  await app.type('/repo');
  assert.equal((await app.query(field))[0].value, '/repo');
  await app.key('keydown', { key: 'Backspace', code: 'Backspace' });
  await app.key('keyup', { key: 'Backspace', code: 'Backspace' });
  assert.equal((await app.query(field))[0].value, '/rep');
  await app.type('o');
  assert.equal((await app.query(save))[0].disabled, false, 'React clears the disabled attribute');
  await assert.rejects(app.click(save), /outside the viewport/);
  await app.wheel({ x: 100, y: 200, deltaY: 400 });
  await app.assert(() => scrollY > 0);
  const button = (await app.query(save))[0];
  await app.eventLog();
  await app.click(save);
  await app.assert(() => globalThis.submits === 1 && document.querySelector('#saved').textContent === '/repo');
  const events = await app.eventLog();
  for (const type of ['mousedown', 'mouseup', 'click']) {
    const event = events.find(event => event.type === type && event.target?.id === 'save');
    assert.ok(event, `${type} targets the scrolled button after React rerender`);
    assert.ok(Math.abs(event.clientY - (button.box.y + button.box.height / 2)) < 1);
  }
  assert.ok(events.some(event => event.type === 'submit' && event.defaultPrevented));
  await app.wheel({ x: 100, y: 200, deltaY: -1000 });
  await app.click('#cover-switch');
  await app.wheel({ x: 100, y: 200, deltaY: 400 });
  await assert.rejects(app.click(save), /covered/);
  await app.assert(() => globalThis.submits === 1);
  await assert.rejects(app.assert(async () => false), /synchronous/);
  await assert.rejects(app.evaluate(async () => 1), /synchronous/);
  await assert.rejects(app.assert(() => globalThis.submits === 99), error => {
    assert.ok(error.screenshot.length > 100);
    assert.ok(error.artifacts);
    return true;
  });
  console.log('Application UI acceptance passed: scroll, React, focus, IME, keyboard, coverage, evidence');
} finally { await app.close(); }
await assert.rejects(app.query('button'), /closed/);
