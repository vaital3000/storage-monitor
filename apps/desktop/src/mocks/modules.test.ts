import { beforeEach, describe, expect, it } from 'vitest';
import {
  cleanupItems,
  modulesList,
  modulesRefresh,
  onModulesState,
  type ModuleView,
} from '../lib/ipc';
import { installIpcMock } from './ipc';
import { setMockModuleFailure, setMockModuleUnavailable, setMockModules } from './modules';

/** Every `modules:state` the mock emits from now on. */
async function record(): Promise<ModuleView[]> {
  const seen: ModuleView[] = [];
  await onModulesState((view) => seen.push(view));
  return seen;
}

/** Resolves once the demo has left `discovering`. */
function settled(): Promise<ModuleView> {
  return new Promise((resolve) => {
    void onModulesState((view) => {
      if (view.status !== 'discovering') resolve(view);
    });
  });
}

beforeEach(() => {
  installIpcMock();
});

describe('the module manager of the mock', () => {
  it('says discovering, then ready, as the real one does', async () => {
    const seen = await record();
    const done = settled();
    const answer = await modulesRefresh();
    expect(answer[0].status).toBe('discovering');
    await done;
    expect(seen.map((view) => view.status)).toEqual(['discovering', 'ready']);
    expect(seen[1].itemCount).toBe(5);
  });

  it('keeps the items of the last discovery that worked when a refresh fails', async () => {
    let done = settled();
    await modulesRefresh();
    await done;
    setMockModuleFailure('demo', 'the daemon did not answer');
    done = settled();
    await modulesRefresh(['demo']);
    const failed = await done;
    expect(failed).toMatchObject({
      status: 'failed',
      reason: 'the daemon did not answer',
      itemCount: 5,
    });
    expect((await cleanupItems()).total).toBe(5);
  });

  it('drops the items of a module that became unavailable', async () => {
    let done = settled();
    await modulesRefresh();
    await done;
    setMockModuleUnavailable('demo', '`touch` is not installed');
    done = settled();
    await modulesRefresh();
    expect(await done).toMatchObject({
      status: 'unavailable',
      reason: '`touch` is not installed',
      itemCount: 0,
    });
    expect((await cleanupItems()).items).toEqual([]);
  });

  it('holds no module at all in a release build', async () => {
    setMockModules(false);
    expect(await modulesList()).toEqual([]);
    expect(await modulesRefresh()).toEqual([]);
  });

  it('ignores an id no module has', async () => {
    const answer = await modulesRefresh(['docker']);
    expect(answer[0].status).toBe('idle');
  });
});
