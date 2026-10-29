// A screenshot of the notes (lib/shots.ts) over the whole window until a
// click or Esc: from the report (`Zoomable`) or the notes' editor.
import { useEffect, useState } from 'react';

/** An image that opens over the whole window when clicked. */
export function Zoomable({ src, alt, className }: { src: string; alt: string; className: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <img
        src={src}
        alt={alt}
        title="Click to enlarge"
        onClick={() => setOpen(true)}
        className={`cursor-zoom-in ${className}`}
      />
      {open && <Lightbox src={src} alt={alt} onClose={() => setOpen(false)} />}
    </>
  );
}

export function Lightbox({ src, alt, onClose }: { src: string; alt: string; onClose: () => void }) {
  useEffect(() => {
    const close = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', close);
    return () => window.removeEventListener('keydown', close);
  }, [onClose]);
  return (
    <div
      role="dialog"
      aria-label={alt}
      onClick={onClose}
      className="fixed inset-0 z-50 flex cursor-zoom-out flex-col items-center justify-center gap-3 bg-black/85 p-6"
    >
      <img src={src} alt={alt} className="max-h-[calc(100%-2rem)] max-w-full object-contain" />
      {alt && <p className="text-sm text-neutral-300">{alt}</p>}
    </div>
  );
}
