// Markdown from a language model or the person, rendered as React
// elements: react-markdown never renders the HTML in it, so a report
// cannot inject markup into the app's window. Links open in the browser.
// Images are the meeting's screenshots only, from `shots` (lib/shots.ts):
// nothing is loaded from elsewhere.
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { openUrl } from '@tauri-apps/plugin-opener';
import { isShot, shotUrl } from '../lib/shots.js';
import { Zoomable } from './Shots.js';

/** `shots`: the folder of the meeting's screenshots. */
export function Markdown({ text, shots }: { text: string; shots?: string }) {
  return (
    <div className="prose-report">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children }) => (
            <a
              href={href}
              onClick={(e) => {
                e.preventDefault();
                if (href && /^https?:\/\//.test(href)) void openUrl(href);
              }}
            >
              {children}
            </a>
          ),
          img: ({ src, alt }) =>
            shots && typeof src === 'string' && isShot(src) ? (
              <Zoomable
                src={shotUrl(shots, src)}
                alt={alt ?? ''}
                className="rounded-md border border-neutral-200 dark:border-neutral-800"
              />
            ) : null,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
