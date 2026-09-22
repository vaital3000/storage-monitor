// The Cleanup screen's view of the modules: their states and the items they hold, kept
// current by the `modules:state` events, with the discovery of every module that never ran
// started when the screen first opens.

import { useQuery, useQueryClient, type UseQueryResult } from '@tanstack/react-query';
import { useCallback, useEffect, useRef } from 'react';
import {
  cleanupItems,
  modulesList,
  modulesRefresh,
  onModulesState,
  type ItemsPage,
  type ModuleView,
} from '../lib/ipc';

export interface ModulesController {
  modules: UseQueryResult<ModuleView[]>;
  items: UseQueryResult<ItemsPage>;
  /** Starts a discovery of `ids` (every module when there are none). */
  refresh: (ids?: string[]) => Promise<void>;
}

/**
 * Both queries have `staleTime: 0`, which is the Activity query's rule and not the tree's.
 * The events that keep them current are heard only while this screen is mounted, and the
 * shell renders one screen at a time: a rediscovery that finishes while the user is on the
 * Explorer says so to nobody. Reading again on every open is what makes that harmless; the
 * two commands are lookups in memory.
 */
export function useModules(): ModulesController {
  const client = useQueryClient();
  const modules = useQuery({ queryKey: ['modules'], queryFn: modulesList, staleTime: 0 });
  const items = useQuery({
    queryKey: ['cleanupItems'],
    queryFn: () => cleanupItems(),
    staleTime: 0,
  });

  const reread = useCallback(() => {
    void client.invalidateQueries({ queryKey: ['modules'] });
    void client.invalidateQueries({ queryKey: ['cleanupItems'] });
  }, [client]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    void onModulesState(reread).then((stop) => {
      if (gone) stop();
      else unlisten = stop;
    });
    return () => {
      gone = true;
      unlisten?.();
    };
  }, [reread]);

  const refresh = useCallback(
    async (ids?: string[]) => {
      await modulesRefresh(ids);
      reread();
    },
    [reread],
  );

  // A module nobody has asked yet is asked once, when the list first arrives. Once per
  // mount: a module that is idle again later is one the backend forgot, which it does not.
  const asked = useRef(false);
  useEffect(() => {
    if (asked.current || modules.data === undefined) return;
    const idle = modules.data.filter((module) => module.status === 'idle').map((m) => m.id);
    asked.current = true;
    if (idle.length > 0) void refresh(idle);
  }, [modules.data, refresh]);

  return { modules, items, refresh };
}
