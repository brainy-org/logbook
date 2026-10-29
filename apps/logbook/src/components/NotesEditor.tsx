// The person's notes, edited as they look: `# ` a heading, `- ` a list,
// screenshots as images (TipTap). What goes in and out is Markdown
// (tiptap-markdown), the notes as the client keeps them and the report
// reads them. A screenshot's image keeps its file name as `src` and is
// drawn from the `shot` scheme (lib/shots.ts); clicked, it opens over the
// whole window.
import { forwardRef, useImperativeHandle, useState } from 'react';
import { EditorContent, useEditor } from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import Image from '@tiptap/extension-image';
import Placeholder from '@tiptap/extension-placeholder';
import { Markdown } from 'tiptap-markdown';
import { isShot, shotUrl } from '../lib/shots.js';
import { Lightbox } from './Shots.js';

export interface NotesHandle {
  /** A screenshot where the cursor is (or was last), on a line of its own. */
  insertShot: (shot: { name: string; caption: string }) => void;
}

interface Props {
  /** Markdown, read once: a new value is a new editor (`key`). */
  initial: string;
  /** Where the screenshots are: `screenshots`, or `meetings/<id>`. */
  folder: string;
  onChange: (markdown: string) => void;
  onBlur?: () => void;
  placeholder: string;
  editable?: boolean;
  autoFocus?: boolean;
}

/** An image whose `src` is a screenshot's file name, drawn from its folder. */
function shotImage(folder: string) {
  return Image.extend({
    addAttributes() {
      return {
        ...this.parent?.(),
        src: {
          default: null,
          // Copied within the editor, the image's address carries its name.
          parseHTML: (el: HTMLElement) => el.getAttribute('data-name') ?? el.getAttribute('src'),
          renderHTML: (attrs: { src?: string | null }) => {
            const name = attrs.src ?? '';
            return isShot(name) ? { src: shotUrl(folder, name), 'data-name': name } : {};
          },
        },
      };
    },
  });
}

export const NotesEditor = forwardRef<NotesHandle, Props>(function NotesEditor(
  { initial, folder, onChange, onBlur, placeholder, editable = true, autoFocus = false },
  ref,
) {
  const [zoomed, setZoomed] = useState<{ src: string; alt: string } | null>(null);
  const editor = useEditor({
    extensions: [
      StarterKit,
      shotImage(folder).configure({ HTMLAttributes: { class: 'shot' } }),
      Placeholder.configure({ placeholder }),
      // A line break typed in the old plain notes stays one.
      Markdown.configure({ html: false, breaks: true, transformPastedText: true }),
    ],
    content: initial,
    editable,
    autofocus: autoFocus ? 'end' : false,
    editorProps: {
      attributes: { class: 'prose-report notes-editor min-h-[50vh] outline-none' },
      handleClickOn: (_view, _pos, node, _nodePos, event) => {
        if (node.type.name !== 'image') return false;
        const img = event.target as HTMLImageElement;
        setZoomed({ src: img.src, alt: String(node.attrs.alt ?? '') });
        return true;
      },
    },
    onUpdate: ({ editor }) => onChange(markdownOf(editor)),
    onBlur: () => onBlur?.(),
  });

  useImperativeHandle(
    ref,
    () => ({
      insertShot: (shot) => {
        editor
          ?.chain()
          .focus()
          .insertContent([
            { type: 'image', attrs: { src: shot.name, alt: shot.caption } },
            { type: 'paragraph' },
          ])
          .run();
      },
    }),
    [editor],
  );

  return (
    <>
      <EditorContent editor={editor} className="flex-1" />
      {zoomed && <Lightbox src={zoomed.src} alt={zoomed.alt} onClose={() => setZoomed(null)} />}
    </>
  );
});

function markdownOf(editor: { storage: Record<string, unknown> }): string {
  const markdown = editor.storage.markdown as { getMarkdown: () => string };
  return markdown.getMarkdown();
}
