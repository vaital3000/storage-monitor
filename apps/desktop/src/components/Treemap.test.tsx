import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { formatBytes } from '../lib/format';
import type { ChildView } from '../lib/ipc';
import { charts, lastChart, registered } from '../test/echarts';
import Treemap from './Treemap';

vi.mock('echarts/core', async () => (await import('../test/echarts')).echartsCoreMock);
vi.mock('echarts/charts', () => ({ TreemapChart: {} }));
vi.mock('echarts/components', () => ({ TooltipComponent: {} }));
vi.mock('echarts/renderers', () => ({ CanvasRenderer: {} }));

interface Cell {
  name: string;
  value: number;
  [key: string]: unknown;
}

interface Series {
  type: string;
  data: Cell[];
  nodeClick: unknown;
  roam: unknown;
  breadcrumb: { show: boolean };
  label: { formatter: (params: unknown) => string };
}

interface Option {
  series: Series[];
  tooltip: { formatter: (params: unknown) => string };
}

const GB = 1e9;

function child(id: number, name: string, size: number, extra: Partial<ChildView> = {}): ChildView {
  return {
    id,
    name,
    kind: 'dir',
    size,
    logicalSize: size,
    fileCount: 1,
    mtime: 0,
    error: null,
    delta: null,
    hasChildren: true,
    ...extra,
  };
}

/** Seventy directories, 70 GB down to 1 GB, in a parent of 100 GB. */
function seventy(): ChildView[] {
  return Array.from({ length: 70 }, (_, i) => child(i + 1, `dir-${i + 1}`, (70 - i) * GB));
}

function series(): Series {
  const option = lastChart().lastOption() as Option;
  expect(option.series).toHaveLength(1);
  return option.series[0];
}

/** Stand-in for the browser's ResizeObserver: records callbacks so a test can fire them. */
const resizeCallbacks: Array<() => void> = [];
class FakeResizeObserver {
  constructor(callback: () => void) {
    resizeCallbacks.push(callback);
  }
  observe = vi.fn();
  disconnect = vi.fn();
}

beforeEach(() => {
  charts.length = 0;
  resizeCallbacks.length = 0;
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('Treemap', () => {
  it('registers the treemap chart, the tooltip and the canvas renderer', () => {
    expect(registered).toHaveLength(3);
  });

  it('draws one treemap series with the top 60 children and an Other cell', () => {
    render(<Treemap items={seventy()} parentSize={100 * GB} onSelect={() => undefined} />);
    const s = series();
    expect(s.type).toBe('treemap');
    expect(s.nodeClick).toBe(false);
    expect(s.roam).toBe(false);
    expect(s.breadcrumb.show).toBe(false);
    expect(s.data).toHaveLength(61);
    expect(s.data[0]).toMatchObject({ name: 'dir-1', value: 70 * GB });
    expect(s.data[59]).toMatchObject({ name: 'dir-60', value: 11 * GB });
    const other = s.data[60];
    expect(other.name).toBe('Other (10 items)');
    expect(other.value).toBe((10 + 9 + 8 + 7 + 6 + 5 + 4 + 3 + 2 + 1) * GB);
  });

  it('draws every child when there are 60 or fewer, largest first, skipping empty ones', () => {
    const items = [
      child(1, 'small', 1 * GB),
      child(2, 'big', 5 * GB),
      child(3, 'empty', 0, { error: 'Operation not permitted (os error 1)' }),
      child(4, 'file', 2 * GB, { kind: 'file', hasChildren: false }),
    ];
    render(<Treemap items={items} parentSize={8 * GB} onSelect={() => undefined} />);
    expect(series().data.map((cell) => cell.name)).toEqual(['big', 'file', 'small']);
  });

  it('selects a directory on click, but not a file nor the Other cell', () => {
    const onSelect = vi.fn();
    const items = [
      ...seventy(),
      child(99, 'movie.mov', 80 * GB, { kind: 'file', hasChildren: false }),
    ];
    render(<Treemap items={items} parentSize={100 * GB} onSelect={onSelect} />);
    const chart = lastChart();
    const cells = series().data;
    expect(cells[0].name).toBe('movie.mov');
    expect(cells[1].name).toBe('dir-1');
    expect(cells[cells.length - 1].name).toBe('Other (11 items)');

    chart.trigger('click', { data: cells[1] });
    expect(onSelect).toHaveBeenCalledWith(1);

    onSelect.mockClear();
    chart.trigger('click', { data: cells[0] });
    chart.trigger('click', { data: cells[cells.length - 1] });
    chart.trigger('click', { data: undefined });
    expect(onSelect).not.toHaveBeenCalled();
  });

  it('labels cells with name and size, a lock for an unreadable directory', () => {
    const items = [
      child(1, 'Library', 95.4 * GB),
      child(2, 'Mail', 1.2 * GB, { error: 'Operation not permitted (os error 1)' }),
    ];
    render(<Treemap items={items} parentSize={100 * GB} onSelect={() => undefined} />);
    const s = series();
    const label = (cell: Cell) =>
      s.label.formatter({ name: cell.name, value: cell.value, data: cell });
    expect(label(s.data[0])).toBe(`Library\n${formatBytes(95.4 * GB)}`);
    expect(label(s.data[1])).toBe(`🔒 Mail\n${formatBytes(1.2 * GB)}`);
  });

  it('shows name, size, share of the parent and delta in the tooltip', () => {
    const items = [child(1, 'Library', 95.4 * GB, { delta: 6.4 * GB }), child(2, 'src', 4.6 * GB)];
    render(<Treemap items={items} parentSize={100 * GB} onSelect={() => undefined} />);
    const option = lastChart().lastOption() as Option;
    const cells = series().data;
    const tip = option.tooltip.formatter({
      name: 'Library',
      value: cells[0].value,
      data: cells[0],
    });
    expect(tip).toContain('Library');
    expect(tip).toContain(formatBytes(95.4 * GB));
    expect(tip).toContain('95.4%');
    expect(tip).toContain('+6.4 GB');
    const plain = option.tooltip.formatter({ name: 'src', value: cells[1].value, data: cells[1] });
    expect(plain).not.toContain('Δ');
  });

  it('escapes names in the tooltip', () => {
    const items = [child(1, '<b>x</b>', 1 * GB)];
    render(<Treemap items={items} parentSize={1 * GB} onSelect={() => undefined} />);
    const option = lastChart().lastOption() as Option;
    const cells = series().data;
    const tip = option.tooltip.formatter({
      name: cells[0].name,
      value: cells[0].value,
      data: cells[0],
    });
    expect(tip).toContain('&lt;b&gt;x&lt;/b&gt;');
    expect(tip).not.toContain('<b>');
  });

  it('says so for an empty directory instead of drawing cells', () => {
    render(<Treemap items={[]} parentSize={0} onSelect={() => undefined} />);
    expect(screen.getByText('Nothing to show')).toBeInTheDocument();
    expect(lastChart().setOption).not.toHaveBeenCalled();
    expect(lastChart().clear).toHaveBeenCalled();
  });

  it('follows its container size and disposes the chart on unmount', () => {
    const { unmount } = render(
      <Treemap items={seventy()} parentSize={100 * GB} onSelect={() => undefined} />,
    );
    const chart = lastChart();
    expect(resizeCallbacks).toHaveLength(1);
    resizeCallbacks[0]();
    expect(chart.resize).toHaveBeenCalledTimes(1);

    unmount();
    expect(chart.dispose).toHaveBeenCalledTimes(1);
  });

  it('redraws when the children change without creating a new chart', () => {
    const { rerender } = render(
      <Treemap items={seventy()} parentSize={100 * GB} onSelect={() => undefined} />,
    );
    const chart = lastChart();
    const before = chart.setOption.mock.calls.length;
    rerender(
      <Treemap items={seventy().slice(0, 3)} parentSize={100 * GB} onSelect={() => undefined} />,
    );
    expect(charts).toHaveLength(1);
    expect(chart.setOption.mock.calls.length).toBe(before + 1);
    expect(series().data).toHaveLength(3);
  });
});
