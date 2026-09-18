// The scan state machine of the UI, fed by the backend's `scan:progress` and `scan:done`
// events.
//
//   idle | done | cancelled | failed --start()--> running --scan:done--> done | cancelled | failed
//
// `scan:progress` only updates a `running` status; `scan:done` is applied whatever the
// state, because it is the only transition out of `running` and it always carries the
// authoritative final status. Every `scan:done` bumps `generation`: tree queries put it
// in their keys, so a rescan refetches while an older result stays cached until then.
// Generations come from one counter for the whole session, so a remounted page never
// reuses a key that still holds the tree of an earlier scan.

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  onScanDone,
  onScanProgress,
  scanCancel,
  scanStart,
  scanStatus,
  type ScanState,
  type ScanStatus,
  type UnlistenFn,
} from '../lib/ipc';

export const IDLE_STATUS: ScanStatus = {
  state: 'idle',
  root: null,
  files: 0,
  dirs: 0,
  bytes: 0,
  errors: 0,
  currentPath: '',
  durationMs: 0,
  error: null,
  hasPrevious: false,
  previousTakenAt: null,
};

export interface ScanController {
  status: ScanStatus;
  /**
   * True once the backend answered the first `scan_status` or an event arrived; the UI
   * shows nothing before.
   */
  ready: boolean;
  /** Bumped on every `scan:done`, unique for the session; part of every tree query key. */
  generation: number;
  /** A tree can be read: the last scan finished or was cancelled after a partial walk. */
  hasResult: boolean;
  /** Cancel was requested and `scan:done` has not arrived yet. */
  cancelling: boolean;
  /** Starts a scan of `root` (default: the home folder). Never throws; failures land in `status`. */
  start: (root?: string) => Promise<void>;
  cancel: () => Promise<void>;
}

let generations = 0;

function nextGeneration(): number {
  generations += 1;
  return generations;
}

/** States that no `scan:progress` event may move out of. */
function isTerminal(state: ScanState): boolean {
  return state === 'done' || state === 'cancelled' || state === 'failed';
}

export function hasScanResult(state: ScanState): boolean {
  return state === 'done' || state === 'cancelled';
}

/**
 * A subscription that could not be made (the backend is unreachable) is reported through
 * the failed `scan_status` reply; there is nothing to unlisten.
 */
function unsubscribed(): UnlistenFn {
  return () => undefined;
}

export function useScan(): ScanController {
  const [status, setStatus] = useState<ScanStatus>(IDLE_STATUS);
  const [ready, setReady] = useState(false);
  const [generation, setGeneration] = useState(nextGeneration);
  const [cancelling, setCancelling] = useState(false);
  // Set once an event or a command reply was applied, so that a slow reply to the initial
  // `scan_status` cannot overwrite fresher state.
  const touched = useRef(false);

  useEffect(() => {
    let active = true;

    const progress = onScanProgress((next) => {
      if (!active) return;
      touched.current = true;
      setStatus((current) => (isTerminal(current.state) ? current : next));
      // A progress event can beat the reply to the initial `scan_status`.
      setReady(true);
    }).catch(unsubscribed);
    const done = onScanDone((final) => {
      if (!active) return;
      touched.current = true;
      setStatus(final);
      setGeneration(nextGeneration());
      setCancelling(false);
      setReady(true);
    }).catch(unsubscribed);
    scanStatus().then(
      (initial) => {
        if (!active || touched.current) return;
        setStatus(initial);
        setReady(true);
      },
      (e: unknown) => {
        if (!active || touched.current) return;
        setStatus({ ...IDLE_STATUS, state: 'failed', error: String(e) });
        setReady(true);
      },
    );

    return () => {
      active = false;
      // The mock may already be cleared when a test unmounts; an unlisten failure is moot.
      void progress.then((unlisten) => unlisten()).catch(() => undefined);
      void done.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  const start = useCallback(async (root?: string) => {
    setCancelling(false);
    try {
      const started = await scanStart(root);
      touched.current = true;
      // A progress event may have arrived while the reply was on its way; it is fresher.
      setStatus((current) => (current.state === 'running' ? current : started));
      setReady(true);
    } catch (e: unknown) {
      touched.current = true;
      // "a scan is already running": the running status on screen is still right.
      setStatus((current) =>
        current.state === 'running' ? current : { ...current, state: 'failed', error: String(e) },
      );
      setReady(true);
    }
  }, []);

  const cancel = useCallback(async () => {
    if (status.state !== 'running') return;
    setCancelling(true);
    try {
      // The reply is still `running`; `scan:done` with `cancelled` follows. Any other
      // reply means the scan ended on its own before the request landed.
      const reply = await scanCancel();
      if (reply.state !== 'running') setCancelling(false);
    } catch {
      setCancelling(false);
    }
  }, [status.state]);

  return {
    status,
    ready,
    generation,
    hasResult: hasScanResult(status.state),
    cancelling,
    start,
    cancel,
  };
}
