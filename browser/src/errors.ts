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
