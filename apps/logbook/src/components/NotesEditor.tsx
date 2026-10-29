// The person's notes, edited as they look, for writing while listening:
// few blocks, each from its Markdown shortcut, no menu. Paragraphs,
// headings 1–3, lists, actions (`[] `), code (left with Enter on an empty
// last line, ⌘Enter or Esc) and screenshots; quotes, rules and strikes are
// left out, they start by mistake when typing fast. A paragraph always
// ends the notes, so the cursor can go below a heading, code or a
// screenshot. What goes in and out is Markdown (tiptap-markdown): the
// notes as the client keeps them, the report reads them and brainy shows
// them. A screenshot keeps its file name as `src`, is drawn from the
// `shot` scheme (lib/shots.ts) with its caption, and opens over the whole
// window on a double click, Space when selected, or its button.
import { forwardRef, useImperativeHandle, useRef, useState } from 'react';
import {
  EditorContent,
  Extension,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  useEditor,
  type Editor,
  type NodeViewProps,
} from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import Heading from '@tiptap/extension-heading';
import CodeBlock from '@tiptap/extension-code-block';
import Image, { type ImageOptions } from '@tiptap/extension-image';
import Placeholder from '@tiptap/extension-placeholder';
import TaskItem from '@tiptap/extension-task-item';
import TaskList from '@tiptap/extension-task-list';
import type { Node as ProseNode } from '@tiptap/pm/model';
import { NodeSelection, Plugin } from '@tiptap/pm/state';
import { Maximize2 } from 'lucide-react';
import { Markdown } from 'tiptap-markdown';
import { isShot, SCREEN_KEYS, shotUrl } from '../lib/shots.js';
import { Lightbox } from './Shots.js';

export interface NotesHandle {
  /**
   * A screenshot where the person is writing: in place of an empty line,
   * or after the block being written without moving the cursor; at the
   * end before anything was written.
   */
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
  /** The shortcuts, under the notes while they are empty. */
  hints?: boolean;
}

type Zoom = { src: string; alt: string };

/** A heading turns back into a paragraph with Backspace at its start. */
const NotesHeading = Heading.extend({
  addKeyboardShortcuts() {
    return {
      ...this.parent?.(),
      Backspace: () => {
        const { $from, empty } = this.editor.state.selection;
        return (
          empty &&
          $from.parentOffset === 0 &&
          $from.parent.type.name === 'heading' &&
          this.editor.commands.setParagraph()
        );
      },
    };
  },
});

/** Code, and the obvious ways out of it. */
const NotesCodeBlock = CodeBlock.extend({
  addKeyboardShortcuts() {
    const inCode = () => this.editor.isActive('codeBlock');
    return {
      ...this.parent?.(),
      'Mod-Enter': () => inCode() && this.editor.commands.exitCode(),
      Escape: () => inCode() && this.editor.commands.exitCode(),
      Tab: () => inCode() && this.editor.commands.insertContent('  '),
      // Enter on an empty last line: that line goes, and so does the cursor.
      Enter: ({ editor }) => {
        const { $from, empty } = editor.state.selection;
        if (!empty || $from.parent.type.name !== 'codeBlock') return false;
        const atEnd = $from.parentOffset === $from.parent.content.size;
        if (!atEnd || !$from.parent.textContent.endsWith('\n')) return false;
        return editor
          .chain()
          .command(({ tr }) => {
            tr.delete($from.pos - 1, $from.pos);
            return true;
          })
          .exitCode()
          .run();
      },
    };
  },
});

/** A paragraph always last: somewhere to write below any block. */
const TrailingParagraph = Extension.create({
  name: 'trailingParagraph',
  addProseMirrorPlugins() {
    return [
      new Plugin({
        appendTransaction: (_transactions, _old, state) =>
          state.doc.lastChild?.type.name === 'paragraph'
            ? null
            : state.tr
                .insert(state.doc.content.size, state.schema.nodes.paragraph!.create())
                .setMeta('addToHistory', false),
      }),
    ];
  },
});

interface ShotOptions extends ImageOptions {
  folder: string;
  onZoom: (zoom: Zoom) => void;
}

/** A screenshot: its image and caption, selected with a click. */
function ShotView({ node, selected, extension }: NodeViewProps) {
  const { folder, onZoom } = extension.options as ShotOptions;
  const name = String(node.attrs.src ?? '');
  const alt = String(node.attrs.alt ?? '');
  const src = shotUrl(folder, name);
  return (
    <NodeViewWrapper as="figure" className={`shot ${selected ? 'is-selected' : ''}`}>
      <div className="group relative inline-block max-w-full" data-drag-handle>
        <img src={src} alt={alt} draggable={false} onDoubleClick={() => onZoom({ src, alt })} />
        <button
          type="button"
          contentEditable={false}
          title="Enlarge (Space)"
          onClick={() => onZoom({ src, alt })}
          className="absolute top-1.5 right-1.5 hidden rounded-md bg-black/60 p-1 text-white group-hover:block"
        >
          <Maximize2 className="size-3.5" />
        </button>
      </div>
      {alt && <figcaption contentEditable={false}>{alt}</figcaption>}
    </NodeViewWrapper>
  );
}

/** An image whose `src` is a screenshot's file name; any other image is left out. */
const ShotImage = Image.extend<ShotOptions>({
  draggable: true,
  addOptions() {
    // Image's own, written out: `this.parent` is not there in addOptions.
    return {
      inline: false,
      allowBase64: false,
      HTMLAttributes: {},
      folder: '',
      onZoom: () => undefined,
    };
  },
  addAttributes() {
    return {
      ...this.parent?.(),
      src: {
        default: null,
        // Copied within the editor, the image's address carries its name.
        parseHTML: (el: HTMLElement) => el.getAttribute('data-name') ?? el.getAttribute('src'),
        renderHTML: (attrs: { src?: string | null }) => {
          const name = attrs.src ?? '';
          return isShot(name) ? { src: shotUrl(this.options.folder, name), 'data-name': name } : {};
        },
      },
    };
  },
  parseHTML() {
    return [
      {
        tag: 'img[src]',
        getAttrs: (el) => {
          const e = el as HTMLElement;
          return isShot(e.getAttribute('data-name') ?? e.getAttribute('src') ?? '') ? null : false;
        },
      },
    ];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ShotView);
  },
  // A block of its own in the Markdown: tiptap-markdown writes an image
  // inline, and the block after it would start on its line.
  addStorage() {
    return {
      markdown: {
        serialize(state: MarkdownState, node: ProseNode) {
          state.write(
            `![${state.esc(String(node.attrs.alt ?? ''))}](${String(node.attrs.src ?? '')})`,
          );
          state.closeBlock(node);
        },
        parse: {},
      },
    };
  },
});

/** What tiptap-markdown hands a node's `serialize` (prosemirror-markdown's state). */
interface MarkdownState {
  write: (text: string) => void;
  esc: (text: string) => string;
  closeBlock: (node: ProseNode) => void;
}

function markdownOf(editor: Editor): string {
  const markdown = editor.storage.markdown as { getMarkdown: () => string };
  return markdown.getMarkdown().trimEnd();
}

export const NotesEditor = forwardRef<NotesHandle, Props>(function NotesEditor(
  {
    initial,
    folder,
    onChange,
    onBlur,
    placeholder,
    editable = true,
    autoFocus = false,
    hints = false,
  },
  ref,
) {
  const [zoomed, setZoomed] = useState<Zoom | null>(null);
  const [empty, setEmpty] = useState(initial.trim() === '');
  // The Markdown last told: the trailing paragraph is no change.
  const told = useRef(initial.trimEnd());
  // Before the person has been in the notes, a screenshot goes at the end.
  const focused = useRef(false);

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        heading: false,
        codeBlock: false,
        blockquote: false,
        horizontalRule: false,
        strike: false,
      }),
      NotesHeading.configure({ levels: [1, 2, 3] }),
      NotesCodeBlock.configure({
        exitOnTripleEnter: false,
        exitOnArrowDown: true,
        HTMLAttributes: { spellcheck: 'false' },
      }),
      TaskList,
      TaskItem.configure({ nested: true }),
      ShotImage.configure({ folder, onZoom: setZoomed }),
      TrailingParagraph,
      Placeholder.configure({
        showOnlyCurrent: true,
        includeChildren: true,
        placeholder: ({ editor, node, pos }) => {
          if (editor.isEmpty) return placeholder;
          if (node.type.name === 'heading') return 'Heading';
          const parent = editor.state.doc.resolve(pos).parent;
          return parent.type.name === 'taskItem' ? 'To do' : '';
        },
      }),
      // A line break typed in the old plain notes stays one.
      Markdown.configure({
        html: false,
        breaks: true,
        tightLists: true,
        transformPastedText: true,
      }),
    ],
    content: initial,
    editable,
    autofocus: autoFocus ? 'end' : false,
    editorProps: {
      attributes: { class: 'notes-editor min-h-[50vh] outline-none' },
      handleKeyDown: (view, event) => {
        const { selection } = view.state;
        if (
          event.key === ' ' &&
          selection instanceof NodeSelection &&
          selection.node.type.name === 'image'
        ) {
          const img = view.nodeDOM(selection.from) as HTMLElement | null;
          const src = img?.querySelector('img')?.src;
          if (src) setZoomed({ src, alt: String(selection.node.attrs.alt ?? '') });
          return true;
        }
        return false;
      },
    },
    // The trailing paragraph before anything is typed, out of the history.
    onCreate: ({ editor }) => {
      editor.view.dispatch(editor.state.tr.setMeta('addToHistory', false));
    },
    onFocus: () => {
      focused.current = true;
    },
    onUpdate: ({ editor }) => {
      setEmpty(editor.isEmpty);
      const markdown = markdownOf(editor);
      if (markdown === told.current) return;
      told.current = markdown;
      onChange(markdown);
    },
    onBlur: () => onBlur?.(),
  });

  useImperativeHandle(
    ref,
    () => ({
      insertShot: (shot) => {
        if (!editor) return;
        const image = { type: 'image', attrs: { src: shot.name, alt: shot.caption } };
        const { state } = editor;
        const $from = focused.current
          ? state.selection.$from
          : state.doc.resolve(state.doc.content.size - 1);
        const blank = $from.parent.type.name === 'paragraph' && $from.parent.content.size === 0;
        if (blank && $from.depth >= 1) {
          // An empty line: the screenshot in its place, the cursor below it.
          editor
            .chain()
            .focus()
            .insertContentAt({ from: $from.before(), to: $from.after() }, [
              image,
              { type: 'paragraph' },
            ])
            .run();
        } else {
          // A line being written: the screenshot after it, the cursor left there.
          const at = $from.depth >= 1 ? $from.after() : state.doc.content.size;
          editor.chain().insertContentAt(at, image, { updateSelection: false }).run();
        }
        window.requestAnimationFrame(() => {
          const added = editor.view.dom.querySelector<HTMLElement>(
            `img[src$="${CSS.escape(encodeURIComponent(shot.name))}"]`,
          );
          const figure = added?.closest('figure');
          figure?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
          figure?.classList.add('just-added');
          window.setTimeout(() => figure?.classList.remove('just-added'), 1000);
        });
      },
    }),
    [editor],
  );

  return (
    <>
      <EditorContent editor={editor} className="flex-1" />
      {hints && empty && editable && (
        <p className="mt-2 font-mono text-xs text-neutral-400">
          -&nbsp; list &nbsp;·&nbsp; []&nbsp; action &nbsp;·&nbsp; #&nbsp; heading &nbsp;·&nbsp;{' '}
          {SCREEN_KEYS}&nbsp; screenshot
        </p>
      )}
      {zoomed && <Lightbox src={zoomed.src} alt={zoomed.alt} onClose={() => setZoomed(null)} />}
    </>
  );
});
