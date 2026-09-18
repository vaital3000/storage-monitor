// Wraps the `invoke` that `mockIPC` installed (the mock's own `fixUnlisten` does the same),
// so a test can hold, replace or watch the reply of one command while the rest of the
// mock keeps answering. `clearMocks()` in the global `afterEach` drops every wrap.

type RawInvoke = (cmd: string, args?: unknown, options?: unknown) => Promise<unknown>;

function bridge(): { invoke: RawInvoke } {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: RawInvoke } })
    .__TAURI_INTERNALS__;
  if (internals?.invoke === undefined) {
    throw new Error('installIpcMock() must run before the invoke is wrapped');
  }
  return internals as { invoke: RawInvoke };
}

/** Routes every command through `wrap`; `original()` runs the handler underneath. */
export function wrapInvoke(
  wrap: (cmd: string, original: () => Promise<unknown>) => Promise<unknown>,
): void {
  const internals = bridge();
  const inner = internals.invoke;
  internals.invoke = (cmd, args, options) => wrap(cmd, () => inner(cmd, args, options));
}

/**
 * Holds every reply to `cmd` until the returned function is called. The mock still handles
 * the command right away, so a held `scan_start` starts the simulated scan.
 */
export function holdReply(cmd: string): () => void {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  wrapInvoke(async (name, original) => {
    const reply = original();
    if (name !== cmd) return reply;
    // Marks a rejection as handled while it waits; the caller still receives it.
    reply.catch(() => undefined);
    await gate;
    return reply;
  });
  return release;
}

/** Answers the next call of `cmd` with what `reply` returns or throws, instead of the mock. */
export function replyOnce(cmd: string, reply: () => unknown): void {
  let pending = true;
  wrapInvoke((name, original) => {
    if (name !== cmd || !pending) return original();
    pending = false;
    return Promise.resolve().then(reply);
  });
}

/** The name of every command invoked from now on, in order. */
export function recordCommands(): string[] {
  const commands: string[] = [];
  wrapInvoke((cmd, original) => {
    commands.push(cmd);
    return original();
  });
  return commands;
}
