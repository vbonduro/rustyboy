/**
 * Shared types used by both app.ts and menu.ts.
 * Imported as `import type` in both files — zero runtime cost.
 */

/** A single item in a canvas menu list. */
export interface MenuItem {
  label: string;
  value: string;
}

/** Options passed to MenuRenderer.show(). */
export interface MenuOptions {
  title?: string;
  items?: MenuItem[];
  /** Footer hint text. Defaults to "▲▼ MOVE  A SELECT  B BACK". */
  footer?: string;
  /** Called when the user confirms a selection (A button / Enter key). */
  onSelect?: (item: MenuItem) => void | Promise<void>;
  /** Called when the user presses B / Escape. Receives current selection index. */
  onBack?: (selIdx?: number) => void;
  /** Called when the user presses the Select button. Receives current selection index. */
  onSelectBtn?: (selIdx: number) => void | Promise<void>;
}

/** Minimal surface of a live MenuRenderer instance visible to app.ts. */
export interface MenuRendererInstance {
  show(opts: MenuOptions): void;
  hide(): void;
  isActive(): boolean;
  handleInput(key: string): void;
  /** Title of the currently showing menu (undefined when hidden). */
  readonly title: string | undefined;
}
