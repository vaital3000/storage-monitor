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
  itemStyle: { color: string };
  label?: { color: string; textBorderWidth: number };
  [key: string]: unknown;
}

interface Series {
  type: string;
  data: Cell[];
  nodeClick: unknown;
  roam: unknown;
  breadcrumb: { show: boolean };
  label: {
    color: string;
    textBorderColor: string;
    textBorderWidth: number;
    formatter: (params: unknown) => string;
  };
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

/** Relative luminance of `#fff` or of an `hsl(h s% l%)` colour, as WCAG 2 defines it. */
function luminance(color: string): number {
  if (color === '#fff') return 1;
  const match = /^hsl\(([\d.]+) ([\d.]+)% ([\d.]+)%\)$/.exec(color);
  if (match === null) throw new Error(`not an hsl colour: ${color}`);
  const [h, s, l] = match.slice(1).map((part) => Number(part) / (part === match[1] ? 1 : 100));
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  const sectors = [
    [c, x, 0],
    [x, c, 0],
    [0, c, x],
    [0, x, c],
    [x, 0, c],
    [c, 0, x],
  ];
  const [r, g, b] = sectors[Math.floor(h / 60) % 6].map((v) => v + m);
  const linear = (v: number) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}

/** WCAG 2 contrast ratio between two colours, 1 to 21. */
function contrast(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}

/** Thirty directories and thirty files with interleaved sizes: sixty cells, no "Other". */
function mixed(): ChildView[] {
  return Array.from({ length: 30 }, (_, i) => [
    child(i + 1, `dir-${i + 1}`, (60 - 2 * i) * GB),
    child(100 + i, `file-${i + 1}`, (59 - 2 * i) * GB, { kind: 'file', hasChildren: false }),
  ]).flat();
}

/** Stand-in for the browser's ResizeObserver: records instances so a test can fire them. */
class FakeResizeObserver {
  constructor(public callback: () => void) {
    observers.push(this);
  }
  observe = vi.fn();
  disconnect = vi.fn();
}
const observers: FakeResizeObserver[] = [];

beforeEach(() => {
  charts.length = 0;
  observers.length = 0;
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
    expect(contrast(s.label.color, other.itemStyle.color)).toBeGreaterThanOrEqual(4.5);
  });

  it('keeps the white labels readable: an outline, and at least 4.5:1 on every cell', () => {
    render(<Treemap items={mixed()} parentSize={2000 * GB} onSelect={() => undefined} />);
    const s = series();
    expect(s.data).toHaveLength(60);
    expect(s.label).toMatchObject({
      color: '#fff',
      textBorderColor: 'rgba(0,0,0,0.35)',
      textBorderWidth: 2,
    });
    for (const cell of s.data) {
      expect(contrast('#fff', cell.itemStyle.color), cell.name).toBeGreaterThanOrEqual(4.5);
    }
    // Directories run blue-grey, files a warm grey: told apart by hue, not by lightness.
    expect(s.data[0].itemStyle.color).toMatch(/^hsl\(215 /);
    expect(s.data[1].itemStyle.color).toMatch(/^hsl\(30 /);
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
    // The unreadable cell stays light, so its label is dark and drops the outline.
    expect(s.data[0].label).toBeUndefined();
    const unreadable = s.data[1].label;
    expect(unreadable).toMatchObject({ textBorderWidth: 0 });
    expect(contrast(unreadable!.color, s.data[1].itemStyle.color)).toBeGreaterThanOrEqual(4.5);
    expect(luminance(s.data[1].itemStyle.color)).toBeGreaterThan(
      luminance(s.data[0].itemStyle.color),
    );
  });

  it('paints a partially read directory like any other, without a lock', () => {
    const error = '3 entries could not be read';
    const partial = [child(1, 'Library', 10 * GB), child(2, 'App Support', 5 * GB, { error })];
    const plain = [child(1, 'Library', 10 * GB), child(2, 'App Support', 5 * GB)];
    render(<Treemap items={partial} parentSize={15 * GB} onSelect={() => undefined} />);
    const withError = series();
    render(<Treemap items={plain} parentSize={15 * GB} onSelect={() => undefined} />);
    const withoutError = series();

    expect(withError.data[1].itemStyle).toEqual(withoutError.data[1].itemStyle);
    expect(withError.data[1].itemStyle).not.toEqual(withError.data[0].itemStyle);
    const label = withError.label.formatter({ data: withError.data[1] });
    expect(label).toBe(`App Support\n${formatBytes(5 * GB)}`);
    // The tooltip still explains it.
    const option = charts[0].lastOption() as Option;
    expect(option.tooltip.formatter({ data: withError.data[1] })).toContain(error);
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

  it('follows its container size and lets go of it with the chart on unmount', () => {
    const { unmount } = render(
      <Treemap items={seventy()} parentSize={100 * GB} onSelect={() => undefined} />,
    );
    const chart = lastChart();
    expect(observers).toHaveLength(1);
    const [observer] = observers;
    expect(observer.observe).toHaveBeenCalledWith(screen.getByTestId('treemap'));
    observer.callback();
    expect(chart.resize).toHaveBeenCalledTimes(1);

    unmount();
    expect(chart.dispose).toHaveBeenCalledTimes(1);
    expect(observer.disconnect).toHaveBeenCalledTimes(1);
  });

  it('clicks reach the latest onSelect without a new chart', () => {
    const first = vi.fn();
    const second = vi.fn();
    const { rerender } = render(
      <Treemap items={seventy()} parentSize={100 * GB} onSelect={first} />,
    );
    rerender(<Treemap items={seventy()} parentSize={100 * GB} onSelect={second} />);
    expect(charts).toHaveLength(1);
    lastChart().trigger('click', { data: series().data[0] });
    expect(second).toHaveBeenCalledWith(1);
    expect(first).not.toHaveBeenCalled();
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
