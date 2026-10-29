// The notes' screenshots (lib/shots.ts): small under the notes, and each,
// clicked, over the whole window until a click or Esc.
import { useEffect, useState } from 'react';
import { shotUrl } from '../lib/shots.js';

/** The notes' screenshots, small, each with its caption. */
export function Shots({
  folder,
  shots,
}: {
  folder: string;
  shots: { name: string; caption: string }[];
}) {
  return (
    <div className="mt-4 grid grid-cols-3 gap-3 max-sm:grid-cols-2">
      {shots.map((s) => (
        <figure key={s.name}>
          <Zoomable
            src={shotUrl(folder, s.name)}
            alt={s.caption}
            className="aspect-video w-full rounded-md border border-neutral-200 object-cover dark:border-neutral-800"
          />
          <figcaption className="mt-1 truncate text-xs text-neutral-500">{s.caption}</figcaption>
        </figure>
      ))}
    </div>
  );
}

/** An image that opens over the whole window when clicked. */
export function Zoomable({ src, alt, className }: { src: string; alt: string; className: string }) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return;
    const close = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', close);
    return () => window.removeEventListener('keydown', close);
  }, [open]);
  return (
    <>
      <img
        src={src}
        alt={alt}
        title="Click to enlarge"
        onClick={() => setOpen(true)}
        className={`cursor-zoom-in ${className}`}
      />
      {open && (
        <div
          role="dialog"
          aria-label={alt}
          onClick={() => setOpen(false)}
          className="fixed inset-0 z-50 flex cursor-zoom-out flex-col items-center justify-center gap-3 bg-black/85 p-6"
        >
          <img src={src} alt={alt} className="max-h-[calc(100%-2rem)] max-w-full object-contain" />
          {alt && <p className="text-sm text-neutral-300">{alt}</p>}
        </div>
      )}
    </>
  );
}
