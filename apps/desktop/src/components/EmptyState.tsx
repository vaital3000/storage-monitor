import { HardDrive } from 'lucide-react';
import Button from './Button';

interface EmptyStateProps {
  /** The folder a scan would walk; undefined until the backend answered. */
  root: string | undefined;
  onScan: () => void;
}

/** What the Explorer shows before the first scan. */
export default function EmptyState({ root, onScan }: EmptyStateProps) {
  return (
    <section className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
      <HardDrive className="size-10 text-neutral-400" strokeWidth={1.5} />
      <h2 className="text-xl font-semibold">Scan your home folder</h2>
      <p className="font-mono text-sm text-neutral-500">{root ?? '…'}</p>
      <Button variant="primary" onClick={onScan} className="mt-2">
        Scan
      </Button>
      <p className="max-w-md text-sm text-neutral-500">
        Scans the home folder. Some folders in Library need Full Disk Access; they are reported, not
        skipped.
      </p>
    </section>
  );
}
