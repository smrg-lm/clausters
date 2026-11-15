// The page's backend events: what the desktop's Rust side emits, emitted here
// (`session-start`, `session-msg`, `session-exit`).

type Handler = (event: { payload: unknown }) => void;

const handlers = new Map<string, Set<Handler>>();

export function listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<() => void> {
  let set = handlers.get(event);
  if (!set) handlers.set(event, (set = new Set()));
  set.add(handler as Handler);
  return Promise.resolve(() => set.delete(handler as Handler));
}

export function emit(event: string, payload: unknown): void {
  for (const handler of [...(handlers.get(event) ?? [])]) handler({ payload });
}
