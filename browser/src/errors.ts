// A refusal or failure the daemon passes on to Chat with Work as a tool
// error: a stable `code` and a sentence for the model and the person.
export class ToolError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

// The person stopped Chat with Work from using the browser, in the
// extension or on the computer.
export function browserStopped(): ToolError {
  return new ToolError(
    "browser_stopped",
    "the person stopped Chat with Work from using the browser; they can resume it on their computer",
  );
}
