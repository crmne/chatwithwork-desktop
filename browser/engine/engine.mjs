// The pinned, unchanged Playwriter engine owns actionability, scrolling,
// hit checking and trusted input. CWW owns the fixed API and local policy.
// See notices/ for the upstream MIT/Apache provenance and registry mismatch.
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
const require = createRequire(import.meta.url);
const root = dirname(require.resolve('@xmorse/playwright-core/package.json'));
const { createPlaywright } = require(join(root, 'lib/server/playwright.js'));
const { CRBrowser } = require(join(root, 'lib/server/chromium/crBrowser.js'));
const { ProgressController } = require(join(root, 'lib/server/progress.js'));

const integer = value => Number.isSafeInteger(value) && value > 0;
export function validate(operation) {
  if (!operation || !['click', 'type', 'select'].includes(operation.action) ||
      !integer(operation.backendNodeId) || typeof operation.session !== 'string' ||
      !operation.params || typeof operation.params !== 'object') throw Error('invalid_operation');
  if (operation.timeout_ms !== undefined && (!integer(operation.timeout_ms) || operation.timeout_ms > 14_000)) throw Error('invalid_operation');
  const allowed = { click: ['tab', 'ref', 'dry_run', 'submits'], type: ['tab', 'ref', 'text', 'dry_run', 'clear', 'submit'], select: ['tab', 'ref', 'values', 'dry_run'] }[operation.action];
  if (Object.keys(operation.params).some(key => !allowed.includes(key))) throw Error('invalid_operation');
  const p = operation.params;
  for (const key of ['dry_run', 'submits', 'clear', 'submit']) if (p[key] !== undefined && typeof p[key] !== 'boolean') throw Error('invalid_operation');
  if (operation.action === 'type' && (typeof p.text !== 'string' || [...p.text].length > 10_000)) throw Error('invalid_operation');
  if (operation.action === 'select' && (!Array.isArray(p.values) || !p.values.length || p.values.length > 50 || p.values.some(value => typeof value !== 'string'))) throw Error('invalid_operation');
  if (!Array.isArray(operation.labelBackendNodeIds) || operation.labelBackendNodeIds.length > 8 || !operation.labelBackendNodeIds.every(integer)) throw Error('invalid_operation');
}

export async function execute(operation, wire, signal) {
  validate(operation);
  let closed = false;
  let navigated = false;
  let surface = operation.backendNodeId;
  const controllers = new Set();
  const transport = {
    send(command) {
      if (closed) return;
      wire.command(command).then(reply => {
        if (closed) return;
        if (reply.error?.code === 'navigation_started') navigated = true;
        transport.onmessage?.({ id: command.id, sessionId: command.sessionId,
          ...(reply.error ? { error: { code: -32000, message: reply.error.code || 'page_error' } } : { result: reply.result }) });
      }).catch(() => {
        if (!closed) transport.onmessage?.({ id: command.id, sessionId: command.sessionId, error: { code: -32000, message: 'browser_unavailable' } });
      });
    },
    close() { if (!closed) { closed = true; transport.onclose?.(); } },
  };
  wire.events(message => {
    if (message.method === 'Cww.navigationStarted') { navigated = true; transport.close(); }
    else if (message.method === 'Cww.operationCancelled') { for (const controller of controllers) void controller.abort(Error('operation_cancelled')); transport.close(); }
    else if (!closed) transport.onmessage?.(message);
  });
  const cancel = () => { for (const controller of controllers) void controller.abort(Error('operation_cancelled')); transport.close(); };
  signal.addEventListener('abort', cancel, { once: true });
  const run = async (task, timeout = 8_000) => {
    if (signal.aborted) throw Error('operation_cancelled');
    const controller = new ProgressController(); controllers.add(controller);
    try { return await controller.run(task, timeout); } finally { controllers.delete(controller); }
  };
  try {
    if (signal.aborted) throw Error('operation_cancelled');
    const browser = await CRBrowser.connect(createPlaywright({ sdkLanguage: 'javascript' }), transport, {
      name: 'chromium', isChromium: true, persistent: { noDefaultViewport: true },
      browserProcess: { close: async () => transport.close(), kill: async () => transport.close() },
      protocolLogger: () => {}, browserLogsCollector: { log() {}, recentLogs: () => [] },
      artifactsDir: tmpdir(), downloadsPath: tmpdir(), tracesDir: tmpdir(), originalLaunchOptions: {},
    });
    const page = browser._defaultContext.pages()[0];
    if (!page || browser._defaultContext.pages().length !== 1) throw Error('invalid_target');
    const adopt = async id => page.delegate._mainFrameSession._adoptBackendNodeId(id, await page.mainFrame()._utilityContext());
    const params = operation.params;
    const trial = params.dry_run === true;
    let handle;
    try {
      if (operation.action === 'click') {
        // Surface selection never presses. Once execution starts there is
        // exactly one upstream action and no fallback to another surface.
        const candidates = [operation.backendNodeId, ...operation.labelBackendNodeIds];
        let last;
        for (const candidate of candidates) {
          const next = await adopt(candidate);
          try {
            const trialPhase = await wire.command({id: 0, method: 'Cww.trialPhase', params: {surfaceBackendNodeId: candidate}, sessionId: operation.session});
            if (trialPhase.error) throw Error(trialPhase.error.code);
            await run(progress => next.click(progress, { trial: true }), candidate === operation.backendNodeId && candidates.length > 1 ? 1_000 : 4_000);
            handle = next; surface = candidate; break;
          } catch (error) { last = error; await next.dispose(); if (signal.aborted || navigated) throw error; }
        }
        if (!handle) throw last || Error('not_actionable');
        const phase = await wire.command({ id: 0, method: 'Cww.actionPhase', params: { surfaceBackendNodeId: surface }, sessionId: operation.session });
        if (phase.error) throw Error(phase.error.code);
        if (!trial) await run(progress => handle.click(progress, {}));
      } else {
        handle = await adopt(surface);
        const phase = await wire.command({ id: 0, method: 'Cww.actionPhase', params: { surfaceBackendNodeId: surface }, sessionId: operation.session });
        if (phase.error) throw Error(phase.error.code);
        if (operation.action === 'type') {
          await run(progress => handle.waitForElementState(progress, 'visible'));
          await run(progress => handle.waitForElementState(progress, 'editable'));
          if (!trial) {
            if (params.clear !== false) await run(progress => handle.fill(progress, params.text, {}));
            else {
              await run(progress => handle.press(progress, process.platform === 'darwin' ? 'Meta+ArrowDown' : 'Control+End', {}));
              await run(progress => page.keyboard.insertText(progress, params.text));
            }
            if (params.submit === true) await run(progress => handle.press(progress, 'Enter', {}));
          }
        } else {
          // Resolve CWW's value-or-label inputs to exact option indices using
          // read-only DOM data; the upstream selectOption API owns mutation.
          const indices = await handle.evaluateExpression(`(node, values) => {
            if (!(node instanceof HTMLSelectElement)) throw Error('not_select');
            if (!node.multiple && values.length > 1) throw Error('single_select');
            return values.map(value => { const index = Array.from(node.options).findIndex(option => option.value === value || option.label.trim() === value || option.text.trim() === value); if (index < 0 || node.options[index].disabled) throw Error('unavailable_option'); return { index }; });
          }`, { isFunction: true }, params.values);
          await run(progress => handle.waitForElementState(progress, 'visible'));
          await run(progress => handle.waitForElementState(progress, 'enabled'));
          if (!trial) await run(progress => handle.selectOption(progress, [], indices, {}));
        }
      }
    } finally { handle?.dispose(); }
    return { surfaceBackendNodeId: surface };
  } catch (error) {
    // Navigation after dispatch is settled by the extension without retrying
    // the requested action or exposing a destination outside local policy.
    if (navigated && !signal.aborted) return { surfaceBackendNodeId: surface };
    throw error;
  } finally { signal.removeEventListener('abort', cancel); transport.close(); }
}
