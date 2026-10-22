import { defineConfig } from "@playwright/test";

// End-to-end tests: a real Chromium with the built extension, the real
// `cww` binary as its native messaging host, and the test standing in for
// the daemon on the control socket.
export default defineConfig({
  testDir: "e2e",
  timeout: 60_000,
  workers: 1,
  reporter: [["list"]],
});
