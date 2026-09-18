import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PARTIAL_READ, fixtureNode, fixtureNodeView } from '../mocks/fixtures';
import NodeTable from './NodeTable';

const noop = () => undefined;

describe('NodeTable', () => {
  it('says how many children there are and how many of them are shown', () => {
    const node = fixtureNodeView(0, 3);
    expect(node.truncated).toBe(true);
    render(<NodeTable node={node} onOpen={noop} onReveal={noop} />);
    expect(within(screen.getByTestId('node-rows')).getAllByRole('row')).toHaveLength(3);
    expect(
      screen.getByText(`${node.childrenTotal} items, showing the first 3`),
    ).toBeInTheDocument();
  });

  it('counts a complete page without a "showing" clause', () => {
    const node = fixtureNodeView(0);
    render(<NodeTable node={node} onOpen={noop} onReveal={noop} />);
    expect(screen.getByText(`${node.childrenTotal} items`)).toBeInTheDocument();
    expect(screen.queryByText(/showing the first/)).not.toBeInTheDocument();
  });

  it('marks a partially read directory with a warning, not a lock', () => {
    const library = fixtureNodeView(fixtureNode('Library').id);
    render(<NodeTable node={library} onOpen={noop} onReveal={noop} />);
    const marker = screen.getByRole('img', { name: PARTIAL_READ });
    expect(marker).toHaveAttribute('data-marker', 'partial');
    expect(marker).toHaveAttribute('title', PARTIAL_READ);
  });
});
