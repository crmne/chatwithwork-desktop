// Private, fixed-operation child protocol. Never a script runner, REPL, CDP
// server or network listener. Model inputs cannot choose modules or methods.
import { execute, validate } from './engine.mjs';
const MAX = 2 * 1024 * 1024;
const TOTAL = 16 * 1024 * 1024;
let received = 0, written = 0;
const pending = new Map();
const controller = new AbortController();
let next = 0, started = false, finished = false, buffer = Buffer.alloc(0), receiveEvent;
const queue = [];
const send = value => {
  if (finished) return;
  const line = JSON.stringify(value) + '\n';
  written += Buffer.byteLength(line);
  if (Buffer.byteLength(line) > MAX || written > TOTAL) { if (value.type !== 'error') return fail('too_large'); return; }
  process.stdout.write(line);
};
const fail = (code = 'page_error') => {
  if (finished) return;
  controller.abort();
  send({ type: 'error', error: { code, message: code === 'invalid_argument' ? 'The browser operation is invalid.' : 'The browser operation did not complete. Read the page again before trying another action.' } });
  finished = true;
  for (const wait of pending.values()) wait.reject(Error('operation_cancelled'));
  pending.clear(); process.stdin.destroy();
};
const wire = {
  command(command) {
    if (controller.signal.aborted || pending.size >= 128 || next >= 512) return Promise.reject(Error('operation_cancelled'));
    const id = ++next;
    return new Promise((resolve, reject) => { pending.set(id, { resolve, reject }); send({ type: 'command', id, command }); });
  },
  events(callback) { receiveEvent = callback; for (const event of queue.splice(0)) callback(event); },
};
async function message(value) {
  if (!value || typeof value !== 'object') return fail('invalid_argument');
  if (value.type === 'cancel') return fail('browser_stopped');
  if (value.type === 'reply') {
    const wait = pending.get(value.id); if (!wait) return fail('invalid_argument');
    pending.delete(value.id); wait.resolve(value); return;
  }
  if (value.type === 'event') {
    if (receiveEvent) receiveEvent(value.message);
    else if (queue.length < 128) queue.push(value.message);
    else fail('too_large');
    return;
  }
  if (value.type !== 'call' || started) return fail('invalid_argument');
  started = true;
  try { validate(value.operation); } catch { return fail('invalid_argument'); }
  setTimeout(() => fail('page_error'), Math.min(14_000, value.operation.timeout_ms ?? 14_000)).unref();
  try {
    const result = await execute(value.operation, wire, controller.signal);
    if (!finished) { send({ type: 'result', result }); finished = true; process.stdin.destroy(); }
  } catch { fail('page_error'); }
}
process.stdin.on('data', chunk => {
  received += chunk.length;
  if (received > TOTAL || buffer.length + chunk.length > MAX) return fail('too_large');
  buffer = Buffer.concat([buffer, chunk]);
  let newline;
  while (!finished && (newline = buffer.indexOf(10)) >= 0) {
    const line = buffer.subarray(0, newline); buffer = buffer.subarray(newline + 1);
    try { void message(JSON.parse(line.toString('utf8'))); } catch { fail('invalid_argument'); }
  }
});
process.stdin.on('end', () => { if (!finished) fail('browser_unavailable'); });
process.stdin.on('error', () => fail('browser_unavailable'));
process.stdout.on('error', () => { controller.abort(); finished = true; process.stdin.destroy(); });
process.on('uncaughtException', () => fail('page_error'));
process.on('unhandledRejection', () => fail('page_error'));
send({ type: 'ready', protocol: 1 });
