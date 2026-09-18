/**
 * The 38 px strip under the hidden native title bar (`titleBarStyle: Overlay`): a drag
 * region with room for the traffic lights on the left. Tauri looks for the attribute on
 * the element under the cursor, so the title carries it too.
 */
export default function TitleBar({ title }: { title: string }) {
  return (
    <header
      data-tauri-drag-region
      className="flex h-[38px] shrink-0 items-center justify-center border-b border-neutral-200 bg-neutral-100 px-20 select-none dark:border-neutral-800 dark:bg-neutral-900"
    >
      <h1
        data-tauri-drag-region
        className="truncate text-sm font-semibold text-neutral-600 dark:text-neutral-300"
      >
        {title}
      </h1>
    </header>
  );
}
