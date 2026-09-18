/** Stands in for the sections that arrive with later phases. */
export default function PlaceholderPage({ title }: { title: string }) {
  return (
    <section className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
      <h2 className="text-xl font-semibold">{title}</h2>
      <p className="text-sm text-muted">Coming in a later phase</p>
    </section>
  );
}
