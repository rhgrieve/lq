// Runs lq off the main thread. A stack overflow inside wasm can leave the module in a bad
// state, so after one the worker reports it and closes; the page starts a new worker.
import init, { check, format, version } from './pkg/lq_wasm.js';

const ready = init().then(
  () => postMessage({ type: 'ready', version: version() }),
  (e) => postMessage({ type: 'failed', message: String(e && e.message || e) }),
);

onmessage = async ({ data }) => {
  await ready;
  const { id, op, source, path, options } = data;
  const t = performance.now();
  try {
    const raw = op === 'format' ? format(source, JSON.stringify(options || {})) : check(source, path);
    postMessage({ type: 'result', id, op, result: JSON.parse(raw), ms: performance.now() - t });
  } catch (e) {
    const tooDeep = e instanceof RangeError;
    postMessage({
      type: 'result', id, op, ms: performance.now() - t,
      error: tooDeep ? 'This file is nested too deeply for the browser build. The native binary handles it.' : String(e && e.message || e),
    });
    postMessage({ type: 'dead' });
    close();
  }
};
