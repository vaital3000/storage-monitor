import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import Button from './Button';

/**
 * `data-variant` exists so that a test can read which variant a control wears — a decision
 * about meaning (the way on, or something that cannot be taken back), where asserting the
 * colour would pin the stylesheet instead. That only works while the attribute and the
 * classes cannot disagree, which is what this file is for: without it, painting every
 * button blue leaves every attribute truthful and every test green.
 */
describe('Button', () => {
  it('says which variant it wears, and wears it', () => {
    render(
      <>
        <Button variant="danger">Delete permanently</Button>
        <Button variant="primary">Move to the Trash</Button>
        <Button>Cancel</Button>
      </>,
    );
    const danger = screen.getByRole('button', { name: 'Delete permanently' });
    const primary = screen.getByRole('button', { name: 'Move to the Trash' });
    const secondary = screen.getByRole('button', { name: 'Cancel' });

    expect(danger).toHaveAttribute('data-variant', 'danger');
    expect(danger).toHaveClass('bg-red-600');
    expect(primary).toHaveAttribute('data-variant', 'primary');
    expect(primary).toHaveClass('bg-blue-600');
    // The default, and the only one of the three that is not a claim about consequences.
    expect(secondary).toHaveAttribute('data-variant', 'secondary');
    expect(secondary).toHaveClass('border');
  });

  it('does not let a caller relabel the variant it is not', () => {
    // A caller's props cannot make a secondary button call itself a danger one: whatever
    // reads this attribute — a test, a future audit — is reading the button, not the call.
    render(<Button {...({ 'data-variant': 'danger' } as Record<string, string>)}>Rescan</Button>);
    expect(screen.getByRole('button', { name: 'Rescan' })).toHaveAttribute(
      'data-variant',
      'secondary',
    );
  });
});
