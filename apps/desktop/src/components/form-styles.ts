// Copied from brainy (apps/web/src/components/form-styles.ts) until it is published in @brainy-org/ui:
// change it there and copy it back, so the two apps stay alike.

// Class names shared by forms: full-size ones for pages (sign-in, settings,
// consent), compact ones (`…SmClass`, `compactFieldClass`) for popovers,
// menus and panel toolbars.

/** A text input or select: one height, one border, an accent focus ring. */
export const fieldClass =
  'h-8 rounded-md border border-neutral-300 bg-white px-2.5 text-sm outline-none transition-shadow focus:border-accent focus:ring-2 focus:ring-accent/20 disabled:opacity-60 dark:border-neutral-600 dark:bg-neutral-900';

/** The one main action of a form or section. */
export const primaryButtonClass =
  'h-8 rounded-md bg-accent-fill px-3 text-sm font-medium text-white hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:opacity-60';

/** Any other action: outlined, so a page has one primary button per section at most. */
export const secondaryButtonClass =
  'h-8 rounded-md border border-neutral-300 bg-white px-3 text-sm font-medium text-neutral-700 hover:bg-neutral-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:opacity-60 dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800';

/** A text input in a popover or menu (a filter value, a name): smaller, the same focus ring. */
export const compactFieldClass =
  'rounded border border-neutral-300 bg-white px-2 py-1 text-sm outline-none focus:border-accent focus:ring-2 focus:ring-accent/20 disabled:opacity-60 dark:border-neutral-600 dark:bg-neutral-900';

/** `primaryButtonClass` in a popover, a menu or a panel's toolbar. */
export const primaryButtonSmClass =
  'h-7 rounded-md bg-accent-fill px-2.5 text-xs font-medium text-white hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:opacity-60';

/** `secondaryButtonClass` in a popover, a menu or a panel's toolbar. */
export const secondaryButtonSmClass =
  'h-7 rounded-md border border-neutral-300 bg-white px-2.5 text-xs font-medium text-neutral-700 hover:bg-neutral-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:opacity-60 dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-200 dark:hover:bg-neutral-800';

/** An irreversible action (delete a workspace): a red fill, in both themes. */
export const dangerButtonClass =
  'h-8 rounded-md bg-red-700 px-3 text-sm font-medium text-white hover:bg-red-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 disabled:cursor-not-allowed disabled:opacity-50';

/** A destructive action that is not the main one (leave, remove, revoke): outlined, red text. */
export const dangerOutlineButtonClass =
  'h-8 rounded-md border border-red-300 bg-white px-3 text-sm font-medium text-red-600 hover:bg-red-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 disabled:cursor-not-allowed disabled:opacity-50 dark:border-red-700 dark:bg-neutral-900 dark:hover:bg-red-950/40';

/**
 * The surface of a toast (a small notice in a corner): dark in both themes,
 * lifted from a dark page by a lighter grey and a hairline. Not inverted:
 * a near-white box on a dark page glares. Position and size are the caller's.
 */
export const toastSurfaceClass =
  'rounded-lg bg-neutral-900 px-4 py-3 text-sm text-white shadow-lg dark:bg-neutral-800 dark:text-neutral-100 dark:ring-1 dark:ring-white/10';

/** An action inside a toast (Reload, Dismiss). */
export const toastButtonClass =
  'rounded border border-white/40 px-2 py-0.5 font-medium hover:bg-white/10';

/**
 * Secondary text (help, descriptions, emails): 7:1 or better on the page in
 * both themes. `neutral-500` is the lightest text grey (5.6:1 on white);
 * `neutral-400` is for icons and placeholders only.
 */
export const mutedTextClass = 'text-neutral-600 dark:text-neutral-400';

/** A destructive action repeated on every row of a list (remove, revoke): quiet until hovered. */
export const quietDangerButtonClass =
  'h-8 rounded-md px-2.5 text-sm font-medium text-neutral-700 hover:bg-red-50 hover:text-red-600 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-500 dark:text-neutral-300 dark:hover:bg-red-950/40';
