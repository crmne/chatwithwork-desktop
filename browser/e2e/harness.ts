// Stands in for the cww daemon on the control socket: the real `cww`
// binary, started by Chromium as the native messaging host, attaches here
// with `browser_attach`, and the test sends the requests the daemon would.

import { chromium, type BrowserContext } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer, type Server as HttpServer } from "node:http";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

// Run from browser/ (npm run e2e).
export const extensionDir = resolve("build");
export const repo = resolve("..");
export const EXTENSION_ID = "ehjfihpogoacjodjnanclodinjnnpmcp";

export function cwwBinary(): string {
  const exe = join(repo, "target", "debug", process.platform === "win32" ? "cww.exe" : "cww");
  if (!existsSync(exe)) {
    execFileSync("cargo", ["build", "--bin", "cww"], { cwd: repo, stdio: "inherit" });
  }
  return exe;
}

export interface Reply {
  result?: any;
  error?: { code: string; message: string };
}

export class FakeDaemon {
  readonly server: Server;
  private socket: Socket | null = null;
  private buffer = "";
  private nextId = 1;
  private waiting = new Map<number, (reply: Reply) => void>();
  readonly events: any[] = [];
  attachedOrigin = "";
  private attachedWaiters: (() => void)[] = [];
  policy: { sites: string[]; stopped: boolean; changes: boolean };

  constructor(
    readonly path: string,
    sites: string[],
  ) {
    this.policy = { sites, stopped: false, changes: false };
    this.server = createServer((socket) => this.accept(socket));
  }

  listen(): Promise<void> {
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    return new Promise((done) => this.server.listen(this.path, done));
  }

  private accept(socket: Socket): void {
    socket.setEncoding("utf8");
    let attached = false;
    socket.on("data", (chunk: string) => {
      this.buffer += chunk;
      let newline: number;
      while ((newline = this.buffer.indexOf("\n")) >= 0) {
        const line = this.buffer.slice(0, newline);
        this.buffer = this.buffer.slice(newline + 1);
        const message = JSON.parse(line);
        if (!attached) {
          if (message.cmd !== "browser_attach") {
            socket.write(`${JSON.stringify({ ok: false, error: "only browser_attach here" })}\n`);
            continue;
          }
          attached = true;
          this.attachedOrigin = message.origin;
          this.socket = socket;
          socket.write(`${JSON.stringify({ ok: true })}\n`);
          this.sendPolicy();
          this.attachedWaiters.splice(0).forEach((w) => w());
          continue;
        }
        if (typeof message.id === "number" && this.waiting.has(message.id)) {
          this.waiting.get(message.id)!(message);
          this.waiting.delete(message.id);
        } else {
          this.events.push(message);
        }
      }
    });
    socket.on("close", () => {
      if (this.socket === socket) this.socket = null;
    });
  }

  attached(): Promise<void> {
    if (this.socket) return Promise.resolve();
    return new Promise((done) => this.attachedWaiters.push(done));
  }

  sendPolicy(): void {
    this.socket?.write(`${JSON.stringify({ method: "policy", params: this.policy })}\n`);
  }

  setPolicy(policy: Partial<{ sites: string[]; stopped: boolean; changes: boolean }>): void {
    this.policy = { ...this.policy, ...policy };
    this.sendPolicy();
  }

  call(method: string, params: Record<string, unknown> = {}): Promise<Reply> {
    if (!this.socket) throw new Error("no extension attached");
    const id = this.nextId++;
    return new Promise((done, fail) => {
      const timer = setTimeout(() => fail(new Error(`${method} got no answer`)), 20_000);
      this.waiting.set(id, (reply) => {
        clearTimeout(timer);
        done(reply);
      });
      this.socket!.write(`${JSON.stringify({ id, method, params })}\n`);
    });
  }

  async waitForEvent(match: (event: any) => boolean, timeout = 10_000): Promise<any> {
    const until = Date.now() + timeout;
    while (Date.now() < until) {
      const found = this.events.find(match);
      if (found) return found;
      await new Promise((r) => setTimeout(r, 50));
    }
    throw new Error("event never came");
  }

  close(): Promise<void> {
    this.socket?.destroy();
    return new Promise((done) => this.server.close(() => done()));
  }
}

// A site to browse, on 127.0.0.1.
export function serve(pages: Record<string, string>): Promise<{ server: HttpServer; origin: string }> {
  const server = createHttpServer((request, response) => {
    const body = pages[request.url ?? "/"];
    if (body === undefined) {
      response.writeHead(404).end("not found");
      return;
    }
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(body);
  });
  return new Promise((done) =>
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : 0;
      done({ server, origin: `http://127.0.0.1:${port}` });
    }),
  );
}

export interface Setup {
  home: string;
  daemon: FakeDaemon;
  context: BrowserContext;
  cleanup: () => Promise<void>;
}

// A throwaway profile with the extension loaded and `cww` registered as its
// native messaging host, pointing at a CWW_HOME of its own.
export async function launch(sites: string[]): Promise<Setup> {
  const home = mkdtempSync(join(tmpdir(), "cww-e2e-"));
  const socket = join(home, "run", "cww.sock");
  const daemon = new FakeDaemon(socket, sites);
  await daemon.listen();
  const context = await launchBrowser(home);
  return {
    home,
    daemon,
    context,
    cleanup: async () => {
      await context.close().catch(() => {});
      await daemon.close().catch(() => {});
      rmSync(home, { recursive: true, force: true });
    },
  };
}

// Chromium with a throwaway profile in `home`, the extension loaded, and
// `cww` as its native messaging host with CWW_HOME=home.
export async function launchBrowser(home: string): Promise<BrowserContext> {
  const profile = join(home, "profile");
  const hosts = join(profile, "NativeMessagingHosts");
  mkdirSync(hosts, { recursive: true });
  const wrapper = join(home, "host.sh");
  writeFileSync(wrapper, `#!/bin/sh\nCWW_HOME='${home}' exec '${cwwBinary()}' "$@"\n`);
  chmodSync(wrapper, 0o755);
  writeFileSync(
    join(hosts, "com.chatwithwork.desktop.json"),
    JSON.stringify({
      name: "com.chatwithwork.desktop",
      description: "Chat with Work Desktop",
      path: wrapper,
      type: "stdio",
      allowed_origins: [`chrome-extension://${EXTENSION_ID}/`],
    }),
  );
  const hardwareCheck = process.env.CWW_BROWSER_GPU_CHECK === "1";
  const context = await chromium.launchPersistentContext(profile, {
    channel: "chromium",
    headless: process.env.HEADED !== "1",
    args: [`--disable-extensions-except=${extensionDir}`, `--load-extension=${extensionDir}`,
      ...(hardwareCheck ? ["--use-angle=gl-egl", "--use-gl=angle"] : [])],
    env: { ...process.env, CWW_HOME: home } as Record<string, string>,
  });
  if (hardwareCheck) {
    const page = context.pages()[0] ?? await context.newPage();
    const renderer = await page.evaluate(() => {
      const gl = document.createElement("canvas").getContext("webgl");
      const ext = gl?.getExtension("WEBGL_debug_renderer_info");
      return ext ? String(gl!.getParameter(ext.UNMASKED_RENDERER_WEBGL)) : "unavailable";
    });
    if (!renderer.includes("NVIDIA") || /SwiftShader|llvmpipe/i.test(renderer)) {
      await context.close();
      throw new Error(`Hardware GPU required, got ${renderer}`);
    }
    console.log(`Browser GPU: ${renderer}`);
  }
  return context;
}

// Run `fn` in the extension's service worker.
export async function inWorker<T>(context: BrowserContext, fn: (arg: any) => Promise<T> | T, arg?: unknown): Promise<T> {
  let worker = context.serviceWorkers().find((w) => w.url().includes(EXTENSION_ID));
  if (!worker) worker = await context.waitForEvent("serviceworker", { predicate: (w) => w.url().includes(EXTENSION_ID) });
  return worker.evaluate(fn as any, arg) as Promise<T>;
}
