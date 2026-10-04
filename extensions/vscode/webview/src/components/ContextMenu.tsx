// Context menu for the Story Map graph — this IS the action panel now.
//
// With the top toolbar removed (limited real estate next to the editor and
// the bottom panel), every story-map action lives here:
//   • node menu   — passage actions (single + multi-selection variants)
//   • pane menu   — creation, view, and layout actions
//
// Items show keyboard hints where they exist; toggle items show a checkmark.

export interface ContextMenuState {
  x: number;
  y: number;
  kind: 'node' | 'pane';
  nodeId?: string;
  /** Graph-canvas coordinates for pane menus ("new passage here"). */
  graphX?: number;
  graphY?: number;
  /** Extra flags for node menus. */
  isStart?: boolean;
  /** Snapshot of the passage selection size at open time. */
  selectionCount?: number;
  /** View-toggle snapshots. */
  minimapOn?: boolean;
  legendOn?: boolean;
}

export interface ContextMenuActions {
  onOpen: (nodeId: string) => void;
  onRename: (nodeId: string) => void;
  onSetStart: (nodeId: string) => void;
  onManualLink: (nodeId: string) => void;
  onEditTags: () => void;
  onEditColor: () => void;
  onEditGroup: () => void;
  onDelete: (nodeId: string) => void;
  onNewPassageHere: (x: number, y: number) => void;
  onNewPassageAtViewCenter: () => void;
  onFitView: () => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onFind: () => void;
  onSaveLayout: () => void;
  onRefresh: () => void;
  onToggleMiniMap: () => void;
  onToggleLegend: () => void;
}

export default function ContextMenu({
  menu,
  actions,
  onClose,
}: {
  menu: ContextMenuState;
  actions: ContextMenuActions;
  onClose: () => void;
}) {
  const item = (
    label: string,
    handler: () => void,
    opts?: { danger?: boolean; hint?: string; disabled?: boolean; checked?: boolean },
  ) => (
    <button
      className={`ctx-item${opts?.danger ? ' ctx-item--danger' : ''}${opts?.checked ? ' ctx-item--checked' : ''}`}
      disabled={opts?.disabled}
      onClick={() => { handler(); onClose(); }}
    >
      <span className="ctx-item__label">
        {opts?.checked !== undefined && (
          <span className="ctx-item__check" aria-hidden="true">
            {opts.checked ? '✓' : ''}
          </span>
        )}
        <span>{label}</span>
      </span>
      {opts?.hint && <span className="ctx-item__hint">{opts.hint}</span>}
    </button>
  );

  const sep = (key: string) => <div className="ctx-sep" key={key} />;
  const label = (text: string, key: string) => (
    <div className="ctx-label" key={key}>{text}</div>
  );

  const multi = (menu.selectionCount ?? 1) > 1;

  return (
    <div
      className="ctx-menu"
      style={{
        left: Math.min(menu.x, window.innerWidth - 240),
        top: Math.min(menu.y, window.innerHeight - 140),
      }}
      onMouseDown={e => e.stopPropagation()}
    >
      {menu.kind === 'node' ? (
        multi ? (
          <>
            {label(`${menu.selectionCount} passages selected`, 'm-h')}
            {item('Open primary in editor', () => actions.onOpen(menu.nodeId!), { hint: 'Enter' })}
            {sep('m-s1')}
            {label('Bulk edit', 'm-b')}
            {item('Tags…', () => actions.onEditTags())}
            {item('Color…', () => actions.onEditColor())}
            {item('Group…', () => actions.onEditGroup())}
            {sep('m-s2')}
            {item(`Delete ${menu.selectionCount} passages`, () => actions.onDelete(menu.nodeId!), { hint: 'Del', danger: true })}
          </>
        ) : (
          <>
            {item('Open in editor', () => actions.onOpen(menu.nodeId!), { hint: 'Click' })}
            {item('Rename…', () => actions.onRename(menu.nodeId!), { hint: 'F2' })}
            {sep('n-s1')}
            {item('Set as start passage', () => actions.onSetStart(menu.nodeId!), {
              disabled: menu.isStart,
            })}
            {item('Manual link…', () => actions.onManualLink(menu.nodeId!))}
            {item('Tags & color…', () => actions.onEditTags())}
            {sep('n-s2')}
            {item('Delete passage', () => actions.onDelete(menu.nodeId!), { hint: 'Del', danger: true })}
          </>
        )
      ) : (
        <>
          {label('Create', 'p-c')}
          {item('New passage here', () => actions.onNewPassageHere(menu.graphX ?? 0, menu.graphY ?? 0), { hint: '2×click' })}
          {item('New passage at view center', () => actions.onNewPassageAtViewCenter())}
          {item('Find passages…', () => actions.onFind(), { hint: '/' })}
          {sep('p-s1')}
          {label('View', 'p-v')}
          {item('Zoom to fit', () => actions.onFitView(), { hint: 'F' })}
          {item('Zoom in', () => actions.onZoomIn(), { hint: 'Scroll' })}
          {item('Zoom out', () => actions.onZoomOut())}
          {item('Select all passages', () => actions.onSelectAll(), { hint: 'Ctrl+A' })}
          {item('Clear selection', () => actions.onClearSelection(), {
            hint: 'Esc',
            disabled: (menu.selectionCount ?? 0) === 0,
          })}
          {sep('p-s2')}
          {label('Layout', 'p-l')}
          {item('Save layout', () => actions.onSaveLayout())}
          {item('Refresh graph', () => actions.onRefresh())}
          {sep('p-s3')}
          {item('Minimap', () => actions.onToggleMiniMap(), { checked: !!menu.minimapOn })}
          {item('Legend', () => actions.onToggleLegend(), { checked: !!menu.legendOn })}
        </>
      )}
    </div>
  );
}
