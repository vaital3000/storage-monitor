import type { ButtonHTMLAttributes } from 'react';

type Variant = 'primary' | 'secondary' | 'danger';

const BASE =
  'inline-flex items-center gap-1.5 rounded-md px-3 py-1 text-sm font-medium transition-colors ' +
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-500 ' +
  'disabled:cursor-default disabled:opacity-50';

const VARIANTS: Record<Variant, string> = {
  primary: 'bg-blue-600 text-white hover:bg-blue-700 disabled:hover:bg-blue-600',
  secondary:
    'border border-neutral-300 bg-white text-neutral-800 hover:bg-neutral-100 ' +
    'disabled:hover:bg-white dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100 ' +
    'dark:hover:bg-neutral-700 dark:disabled:hover:bg-neutral-800',
  // For a button that destroys something and cannot be taken back. Its own colour rather
  // than `primary` in red, because the two are never the same choice on one screen: the
  // emphasis of `primary` says "this is the way on", and this one says "be sure".
  danger: 'bg-red-600 text-white hover:bg-red-700 disabled:hover:bg-red-600',
};

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant };

export default function Button({
  variant = 'secondary',
  className = '',
  type = 'button',
  ...rest
}: ButtonProps) {
  return <button type={type} className={`${BASE} ${VARIANTS[variant]} ${className}`} {...rest} />;
}
