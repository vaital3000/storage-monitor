// A stand-in for `echarts/core` under jsdom, which has no canvas: `init` hands out a
// chart that records its options and lets a test fire the events it subscribed to.
//
//   vi.mock('echarts/core', async () => (await import('../test/echarts')).echartsCoreMock);
//   vi.mock('echarts/charts', () => ({ TreemapChart: {} }));
//   vi.mock('echarts/components', () => ({ TooltipComponent: {} }));
//   vi.mock('echarts/renderers', () => ({ CanvasRenderer: {} }));

import { vi, type Mock } from 'vitest';

type Handler = (params: unknown) => void;

export interface FakeChart {
  setOption: Mock<(option: unknown, opts?: unknown) => void>;
  on: Mock<(event: string, handler: Handler) => void>;
  off: Mock<(event: string, handler?: Handler) => void>;
  resize: Mock<() => void>;
  clear: Mock<() => void>;
  dispose: Mock<() => void>;
  /** Fires every handler registered for `event`, like a user interaction would. */
  trigger: (event: string, params: unknown) => void;
  /** The option passed to the last `setOption`. */
  lastOption: () => unknown;
}

/** Every chart `init` created, oldest first; tests reset it in `beforeEach`. */
export const charts: FakeChart[] = [];

export function createFakeChart(): FakeChart {
  const handlers = new Map<string, Handler[]>();
  const chart: FakeChart = {
    setOption: vi.fn(),
    on: vi.fn((event: string, handler: Handler) => {
      handlers.set(event, [...(handlers.get(event) ?? []), handler]);
    }),
    off: vi.fn(),
    resize: vi.fn(),
    clear: vi.fn(),
    dispose: vi.fn(),
    trigger: (event, params) => {
      for (const handler of handlers.get(event) ?? []) handler(params);
    },
    lastOption: () => {
      const calls = chart.setOption.mock.calls;
      return calls.length > 0 ? calls[calls.length - 1][0] : undefined;
    },
  };
  return chart;
}

/**
 * What `use` registered. Kept in a plain array because Vitest clears the call history a
 * mock recorded during module import before the first test runs.
 */
export const registered: unknown[] = [];

export const echartsCoreMock = {
  init: vi.fn(() => {
    const chart = createFakeChart();
    charts.push(chart);
    return chart;
  }),
  use: vi.fn((modules: unknown) => {
    registered.push(...(Array.isArray(modules) ? modules : [modules]));
  }),
};

export function lastChart(): FakeChart {
  const chart = charts[charts.length - 1];
  if (chart === undefined) {
    throw new Error('no chart was initialised');
  }
  return chart;
}
