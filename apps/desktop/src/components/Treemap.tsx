// The treemap of one directory, drawn by ECharts on a canvas. Only the pieces the chart
// needs are registered (design section 10 keeps ECharts behind this component so it can
// be swapped).

import { TreemapChart, type TreemapSeriesOption } from 'echarts/charts';
import { TooltipComponent, type TooltipComponentOption } from 'echarts/components';
import {
  init,
  use as registerEcharts,
  type ComposeOption,
  type ECElementEvent,
  type EChartsType,
} from 'echarts/core';
import { CanvasRenderer } from 'echarts/renderers';
import { useEffect, useMemo, useRef } from 'react';
import { countLabel, formatBytes, formatDelta, formatPercent } from '../lib/format';
import type { ChildView, NodeId, NodeKind } from '../lib/ipc';
import { describeNodeError } from '../lib/nodeErrors';

// Aliased: ESLint's rules-of-hooks takes a bare `use()` for React's hook.
registerEcharts([TreemapChart, TooltipComponent, CanvasRenderer]);

type TreemapOption = ComposeOption<TreemapSeriesOption | TooltipComponentOption>;
type SeriesDatum = NonNullable<TreemapSeriesOption['data']>[number];

/** One cell: the ECharts datum plus what the click handler and the tooltip need. */
type Cell = SeriesDatum & {
  name: string;
  value: number;
  /** Null for the aggregated "Other" cell. */
  nodeId: NodeId | null;
  /** `aggregate` for the "Other" cell, which stands for many children. */
  kind: NodeKind | 'aggregate';
  delta: number | null;
  error: string | null;
};

/** Cells drawn individually; the rest is one "Other" cell so labels stay readable. */
export const TREEMAP_LIMIT = 60;
const HEIGHT_CLASS = 'h-80';
const OTHER_COLOR = 'hsl(0 0% 70%)';
const UNREADABLE_COLOR = 'hsl(215 18% 78%)';

interface TreemapProps {
  /** The children of the directory on screen, in any order. */
  items: readonly ChildView[];
  /** Size of that directory, for the share shown in tooltips. */
  parentSize: number;
  /** A directory cell was clicked. */
  onSelect: (id: NodeId) => void;
}

function unreadable(error: string | null): boolean {
  return error !== null && describeNodeError(error).kind === 'lock';
}

/** Directories run through a blue-grey ramp by rank, files through a warm grey. */
function cellColor(child: ChildView, rank: number, count: number): string {
  if (unreadable(child.error)) return UNREADABLE_COLOR;
  const t = count > 1 ? rank / (count - 1) : 0;
  return child.kind === 'dir'
    ? `hsl(215 32% ${(44 + t * 18).toFixed(1)}%)`
    : `hsl(30 8% ${(56 + t * 12).toFixed(1)}%)`;
}

function buildCells(items: readonly ChildView[]): Cell[] {
  const sized = items.filter((child) => child.size > 0).sort((a, b) => b.size - a.size);
  const top = sized.slice(0, TREEMAP_LIMIT);
  const rest = sized.slice(TREEMAP_LIMIT);
  const cells: Cell[] = top.map((child, rank) => ({
    name: child.name,
    value: child.size,
    nodeId: child.id,
    kind: child.kind,
    delta: child.delta,
    error: child.error,
    itemStyle: { color: cellColor(child, rank, top.length) },
    cursor: child.kind === 'dir' ? 'pointer' : 'default',
  }));
  if (rest.length > 0) {
    cells.push({
      name: `Other (${countLabel(rest.length, 'item')})`,
      value: rest.reduce((sum, child) => sum + child.size, 0),
      nodeId: null,
      kind: 'aggregate',
      delta: null,
      error: null,
      itemStyle: { color: OTHER_COLOR },
      cursor: 'default',
    });
  }
  return cells;
}

function toCell(data: unknown): Cell | null {
  return typeof data === 'object' && data !== null && 'nodeId' in data ? (data as Cell) : null;
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (char) => {
    switch (char) {
      case '&':
        return '&amp;';
      case '<':
        return '&lt;';
      case '>':
        return '&gt;';
      case '"':
        return '&quot;';
      default:
        return '&#39;';
    }
  });
}

function buildOption(cells: Cell[], parentSize: number): TreemapOption {
  return {
    animation: false,
    tooltip: {
      trigger: 'item',
      confine: true,
      formatter: (params) => {
        const cell = toCell((Array.isArray(params) ? params[0] : params).data);
        if (cell === null) return '';
        const lines = [
          `<strong>${escapeHtml(cell.name)}</strong>`,
          `${formatBytes(cell.value)} · ${formatPercent(cell.value, parentSize)} of parent`,
        ];
        if (cell.delta !== null && cell.delta !== 0) {
          lines.push(`Δ ${formatDelta(cell.delta)} since the previous scan`);
        }
        if (cell.error !== null) {
          lines.push(escapeHtml(describeNodeError(cell.error).title));
        }
        return lines.join('<br>');
      },
    },
    series: [
      {
        type: 'treemap',
        data: cells,
        roam: false,
        nodeClick: false,
        breadcrumb: { show: false },
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
        sort: 'desc',
        itemStyle: { borderWidth: 0, gapWidth: 2, borderRadius: 3 },
        label: {
          show: true,
          position: 'insideTopLeft',
          padding: 6,
          color: '#fff',
          fontSize: 12,
          lineHeight: 16,
          overflow: 'truncate',
          formatter: (params) => {
            const cell = toCell(params.data);
            if (cell === null) return '';
            const lock = unreadable(cell.error) ? '🔒 ' : '';
            return `${lock}${cell.name}\n${formatBytes(cell.value)}`;
          },
        },
        emphasis: {
          focus: 'none',
          itemStyle: { borderColor: 'rgba(255, 255, 255, 0.7)', borderWidth: 1 },
        },
      },
    ],
  };
}

/**
 * Top 60 children by size plus an "Other" cell, 320 px tall. Clicking a directory cell
 * navigates into it; the chart follows its container and is disposed on unmount.
 */
export default function Treemap({ items, parentSize, onSelect }: TreemapProps) {
  const container = useRef<HTMLDivElement>(null);
  const chart = useRef<EChartsType | null>(null);
  const select = useRef(onSelect);

  useEffect(() => {
    select.current = onSelect;
  }, [onSelect]);

  useEffect(() => {
    const element = container.current;
    if (element === null) return;
    const instance = init(element, undefined, { renderer: 'canvas' });
    instance.on('click', (params: ECElementEvent) => {
      const cell = toCell(params.data);
      if (cell !== null && cell.kind === 'dir' && cell.nodeId !== null) {
        select.current(cell.nodeId);
      }
    });
    chart.current = instance;
    const observer =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => instance.resize());
    observer?.observe(element);
    return () => {
      observer?.disconnect();
      instance.dispose();
      chart.current = null;
    };
  }, []);

  const cells = useMemo(() => buildCells(items), [items]);

  useEffect(() => {
    const instance = chart.current;
    if (instance === null) return;
    if (cells.length === 0) {
      instance.clear();
      return;
    }
    instance.setOption(buildOption(cells, parentSize), { notMerge: true });
  }, [cells, parentSize]);

  return (
    <div
      className={`relative ${HEIGHT_CLASS} w-full overflow-hidden rounded-lg border border-neutral-200 bg-neutral-50 dark:border-neutral-800 dark:bg-neutral-900`}
    >
      <div ref={container} data-testid="treemap" className="h-full w-full" />
      {cells.length === 0 && (
        <p className="absolute inset-0 flex items-center justify-center text-sm text-neutral-500">
          Nothing to show
        </p>
      )}
    </div>
  );
}
